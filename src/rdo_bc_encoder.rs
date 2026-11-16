//! Whole images to blocks, with the rate-distortion pass: the port of `rdo_bc_encoder.cpp` and
//! `rdo_bc_encoder.h` (Richard Geldreich, Jr.), bc7enc_rdo's own encoder of images.
//!
//! [`RdoBcEncoder`] takes an [`Image`], pads it to whole blocks, packs every block as BC1, BC3,
//! BC4, BC5 or BC7 (and BC2, which the C++ does not have: see [`BlockFormat::Bc2`])
//! and, when [`RdoBcParams::rdo_lambda`] is above 0, puts the packed blocks
//! through the entropy reduction transform ([`crate::ert`]).
//!
//! The C++ shares the work out with OpenMP; this does with the standard library's threads, on
//! all the machine's cores unless [`RdoBcParams::threads`] says otherwise. The blocks do not
//! depend on how many threads there are. They do depend, as in the C++, on
//! [`RdoBcParams::rdo_max_threads`]: an image of 4,096 blocks or more is put through the
//! rate-distortion pass in that many strips, each on its own.
//!
//! Left out: the C++'s status output (its `printf` of settings, timings and histograms; the
//! numbers are here as [`RdoBcEncoder::total_modified`] and
//! [`RdoBcEncoder::bc7_mode_histogram`]), the two pictures of the smooth-block mask it saves
//! with its debug output, and the second unpacking of BC7 with `bc7decomp_ref` that its
//! `unpack_blocks` checks the first against.

use crate::bc2;
use crate::bc7decomp::{self, ColorRgba};
use crate::bc7e::{self, Bc7e};
use crate::bc7enc::{self, Bc7Enc};
use crate::ert::{self, ReduceEntropyParams};
use crate::image::Image;
use crate::parallel;
use crate::rgbcx::{self, Bc1ApproxMode, Rgbcx};
use std::sync::atomic::{AtomicU32, Ordering};

/// The block formats the encoder makes: the C++'s `m_dxgi_format`, and BC2, which the C++
/// does not make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockFormat {
    /// RGB, 8 bytes a block.
    Bc1,
    /// RGB with alpha of four bits a pixel, 16 bytes a block ([`crate::bc2`]). The colours
    /// are packed as BC3's at [`RdoBcParams::bc1_quality_level`], and the rate-distortion pass
    /// goes over the colours as it does for BC3 and over the alphas as it does over BC3's.
    Bc2,
    /// RGB with alpha, 16 bytes a block.
    Bc3,
    /// One channel, 8 bytes a block.
    Bc4,
    /// Two channels, 16 bytes a block.
    Bc5,
    /// RGB or RGBA, 16 bytes a block.
    Bc7,
}

impl BlockFormat {
    /// The bytes of a block.
    pub fn bytes_per_block(self) -> u32 {
        match self {
            BlockFormat::Bc1 | BlockFormat::Bc4 => 8,
            BlockFormat::Bc2 | BlockFormat::Bc3 | BlockFormat::Bc5 | BlockFormat::Bc7 => 16,
        }
    }

    /// The format's number in DXGI (`DXGI_FORMAT_BC1_UNORM` and the rest).
    pub fn dxgi_format(self) -> u32 {
        match self {
            BlockFormat::Bc1 => 71,
            BlockFormat::Bc2 => 74,
            BlockFormat::Bc3 => 77,
            BlockFormat::Bc4 => 80,
            BlockFormat::Bc5 => 83,
            BlockFormat::Bc7 => 98,
        }
    }
}

/// The settings of [`RdoBcEncoder`]: `rdo_bc_params`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdoBcParams {
    /// How hard the BC7 encoder tries: 0 to 4 for bc7enc; for bc7e, its seven profiles from
    /// `ultrafast` (0) to `slowest` (6 and above).
    pub bc7_uber_level: u32,
    /// The partitions bc7enc scans, up to 64.
    pub bc7enc_max_partitions_to_scan: u32,
    /// Measure BC7's error as the eye sees it. Switched off when the rate-distortion pass is on.
    pub perceptual: bool,
    /// Turn the image upside down first.
    pub y_flip: bool,
    /// The channel BC4 packs, and the first of BC5's two.
    pub bc45_channel0: u32,
    /// The second of BC5's two channels.
    pub bc45_channel1: u32,
    /// How the GPU the BC1 and BC3 blocks are for interpolates.
    pub bc1_mode: Bc1ApproxMode,
    /// Let BC1 use blocks of three colours.
    pub use_bc1_3color_mode: bool,
    /// Let BC1 use the transparent texel of a three-colour block for black.
    pub use_bc1_3color_mode_for_black: bool,
    /// BC1's (and BC3's colour's) quality level, 0 to 18.
    pub bc1_quality_level: u32,
    /// The format to pack: `m_dxgi_format`.
    pub format: BlockFormat,
    /// The weight of bits against error in the rate-distortion pass. 0 leaves the pass out.
    pub rdo_lambda: f32,
    /// Have the entropy reduction transform print its statistics.
    pub rdo_debug_output: bool,
    /// The most a smooth block's error is scaled up by, when `custom` below is set.
    pub rdo_smooth_block_error_scale: f32,
    /// Use the scale above; otherwise one is worked out from lambda and the format.
    pub custom_rdo_smooth_block_error_scale: bool,
    /// The bytes the rate-distortion pass looks back for matches.
    pub lookback_window_size: u32,
    /// (Set by bc7enc_rdo's command line when the window is given; the encoder does not read
    /// it.)
    pub custom_lookback_window_size: bool,
    /// With `bc7enc_reduce_entropy`: quantize mode 6's endpoints.
    pub bc7enc_rdo_bc7_quant_mode6_endpoints: bool,
    /// With `bc7enc_reduce_entropy`: weigh the error of mode 6 lower, so it is chosen more.
    pub bc7enc_rdo_bc7_weight_modes: bool,
    /// With `bc7enc_reduce_entropy`: slightly prefer the low-frequency partitions.
    pub bc7enc_rdo_bc7_weight_low_frequency_partitions: bool,
    /// With `bc7enc_reduce_entropy`: favour p-bits of 0.
    pub bc7enc_rdo_bc7_pbit1_weighting: bool,
    /// A block counts as smooth under this standard deviation.
    pub rdo_max_smooth_block_std_dev: f32,
    /// Let the pass copy bytes to another offset in a block. Much slower.
    pub rdo_allow_relative_movement: bool,
    /// Let the pass try a second copy into a block.
    pub rdo_try_2_matches: bool,
    /// Find the image's very smooth regions and protect them from the pass.
    pub rdo_ultrasmooth_block_handling: bool,
    /// Use the high-quality BC4 encoder for BC3's alpha, BC4 and BC5.
    pub use_hq_bc345: bool,
    /// How far the high-quality BC4 encoder searches.
    pub bc345_search_rad: u32,
    /// Which BC4 modes it may use ([`rgbcx::BC4_USE_ALL_MODES`]).
    pub bc345_mode_mask: u32,
    /// Have bc7enc use mode 6 only.
    pub bc7enc_mode6_only: bool,
    /// Do the rate-distortion pass in strips (see `rdo_max_threads`).
    pub rdo_multithreading: bool,
    /// Set bc7enc up to make blocks that the rate-distortion pass does well with.
    pub bc7enc_reduce_entropy: bool,
    /// Pack BC7 with bc7e (the thorough encoder). False: with bc7enc.
    pub use_bc7e: bool,
    /// The number of strips an image of 4,096 blocks or more is cut into for the
    /// rate-distortion pass, each done on its own. It changes the blocks: a strip cannot copy
    /// from the one before. (bc7enc_rdo's command line sets it from the machine's cores; left
    /// alone it is 128 everywhere.)
    pub rdo_max_threads: u32,
    /// The threads to work on, 0 for all the machine has. It does not change the blocks. (The
    /// crate's own setting.)
    pub threads: u32,
}

impl Default for RdoBcParams {
    fn default() -> Self {
        Self::new()
    }
}

impl RdoBcParams {
    /// The C++'s defaults, as built with bc7e.
    pub fn new() -> Self {
        RdoBcParams {
            bc7_uber_level: 6,
            bc7enc_max_partitions_to_scan: bc7enc::MAX_PARTITIONS,
            perceptual: false,
            y_flip: false,
            bc45_channel0: 0,
            bc45_channel1: 1,
            bc1_mode: Bc1ApproxMode::Ideal,
            use_bc1_3color_mode: true,
            // We're just turning this on by default now, like
            // NVDXT.EXE used to do back in the old original Xbox days.
            use_bc1_3color_mode_for_black: true,
            bc1_quality_level: rgbcx::MAX_LEVEL,
            format: BlockFormat::Bc7,
            rdo_lambda: 0.0,
            rdo_debug_output: false,
            rdo_smooth_block_error_scale: 15.0,
            custom_rdo_smooth_block_error_scale: false,
            lookback_window_size: 128,
            custom_lookback_window_size: false,
            bc7enc_rdo_bc7_quant_mode6_endpoints: true,
            bc7enc_rdo_bc7_weight_modes: true,
            bc7enc_rdo_bc7_weight_low_frequency_partitions: true,
            bc7enc_rdo_bc7_pbit1_weighting: true,
            rdo_max_smooth_block_std_dev: 18.0,
            rdo_allow_relative_movement: false,
            rdo_try_2_matches: true,
            rdo_ultrasmooth_block_handling: true,
            use_hq_bc345: true,
            bc345_search_rad: 5,
            bc345_mode_mask: rgbcx::BC4_USE_ALL_MODES,
            bc7enc_mode6_only: false,
            rdo_multithreading: true,
            bc7enc_reduce_entropy: false,
            // By default, if they've compiled in BC7E.ispc, then use
            // that. In a rate distortion sense it's better overall.
            // https://richg42.blogspot.com/2021/02/average-rate-distortion-curves-for.html
            use_bc7e: true,
            rdo_max_threads: 128,
            threads: 0,
        }
    }

    /// Back to the defaults.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

fn clamp(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a + (b - a) * s
}

/// `std::min` of floats: the second when it is less than the first.
fn min_f(a: f32, b: f32) -> f32 {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max` of floats: the second when the first is less than it.
fn max_f(a: f32, b: f32) -> f32 {
    if a < b {
        b
    } else {
        a
    }
}

/// `color_quad_u8::get_luma`, with REC 709's weights.
fn luma(pixel: &ColorRgba) -> u32 {
    (13938 * u32::from(pixel[0]) + 46869 * u32::from(pixel[1]) + 4729 * u32::from(pixel[2]) + 32768)
        >> 16
}

/// The mask of blocks the C++ calls `ultrasmooth_blocks_vis`: a byte a block, and its flood
/// fill (`image_u8::flood_fill`, after Graphics Gems' SeedFill.c).
struct BlockMask {
    width: i32,
    height: i32,
    values: Vec<u8>,
}

impl BlockMask {
    fn get_clamped(&self, x: i32, y: i32) -> u8 {
        self.values
            [(x.clamp(0, self.width - 1) + y.clamp(0, self.height - 1) * self.width) as usize]
    }

    fn is_inside(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && x < self.width
            && y < self.height
            && self.values[(x + y * self.width) as usize] == 0
    }

    /// Fills the region of 0 that holds a place with 255; the places filled are added to a
    /// list and counted.
    fn flood_fill(&mut self, mut x: i32, mut y: i32, set_pixels: &mut Vec<(i32, i32)>) -> u32 {
        let mut total_set = 0u32;

        if !self.is_inside(x, y) {
            return 0;
        }

        // (The C++ keeps a segment's four numbers in 16 bits each.)
        let mut stack: Vec<[i16; 4]> = Vec::with_capacity(64);
        let height = self.height;
        let push = |stack: &mut Vec<[i16; 4]>, y: i32, xl: i32, xr: i32, dy: i32| {
            if y + dy >= 0 && y + dy < height {
                stack.push([y as i16, xl as i16, xr as i16, dy as i16]);
            }
        };

        push(&mut stack, y, x, x, 1);
        push(&mut stack, y + 1, x, x, -1);

        while let Some(segment) = stack.pop() {
            let (x1, x2, dy) = (
                i32::from(segment[1]),
                i32::from(segment[2]),
                i32::from(segment[3]),
            );
            y = i32::from(segment[0]) + dy;

            x = x1;
            while x >= 0 && self.is_inside(x, y) {
                self.values[(x + y * self.width) as usize] = 255;
                total_set += 1;
                set_pixels.push((x, y));
                x -= 1;
            }

            let mut l = 0;
            // (The C++ jumps into the middle of the loop below when nothing was filled.)
            let mut skip = x >= x1;
            if !skip {
                l = x + 1;
                if l < x1 {
                    push(&mut stack, y, l, x1 - 1, -dy);
                }
                x = x1 + 1;
            }

            loop {
                if !skip {
                    while x < self.width && self.is_inside(x, y) {
                        self.values[(x + y * self.width) as usize] = 255;
                        total_set += 1;
                        set_pixels.push((x, y));
                        x += 1;
                    }
                    push(&mut stack, y, l, x - 1, dy);

                    if x > x2 + 1 {
                        push(&mut stack, y, x2 + 1, x - 1, -dy);
                    }
                }
                skip = false;

                x += 1;
                while x <= x2 && !self.is_inside(x, y) {
                    x += 1;
                }

                l = x;
                if x > x2 {
                    break;
                }
            }
        }

        total_set
    }
}

/// For each block, the scale the rate-distortion pass gives its error when the block lies in a
/// large, very smooth region of the image (120), or -1 for the pass to work one out.
fn compute_block_mse_scales(
    source_image: &Image,
    blocks_x: u32,
    blocks_y: u32,
    total_blocks: u32,
    rdo_debug_output: bool,
) -> Vec<f32> {
    const ULTRASMOOTH_BLOCK_STD_DEV_THRESHOLD: f32 = 2.9;
    const DARK_THRESHOLD: f32 = 13.0;
    const BRIGHT_THRESHOLD: f32 = 222.0;
    const ULTRAMOOTH_BLOCK_MSE_SCALE: f32 = 120.0;
    const ULTRASMOOTH_REGION_TOO_SMALL_THRESHOLD: u32 = 64;

    let mut vis = BlockMask {
        width: blocks_x as i32,
        height: blocks_y as i32,
        values: vec![0u8; total_blocks as usize],
    };

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let block_pixels = source_image.get_block(bx, by);

            let (mut y_num, mut y_total) = (0u32, 0u64);
            for pixel in &block_pixels {
                y_num += 1;
                y_total += u64::from(luma(pixel));
            }

            let max_std_dev = ert::compute_block_max_std_dev(&block_pixels, 4, 4, 3);

            let mut yl = max_std_dev / ULTRASMOOTH_BLOCK_STD_DEV_THRESHOLD;
            yl = clamp(yl, 0.0, 1.0);
            yl *= yl;

            let y_avg = y_total as f32 / y_num as f32;

            if y_avg < DARK_THRESHOLD || y_avg >= BRIGHT_THRESHOLD {
                yl = 1.0;
            }

            let k = ((yl * 255.0 + 0.5) as i32).min(255);

            vis.values[(bx + by * blocks_x) as usize] = k as u8;
        }
    }

    {
        let mut next = vis.values.clone();
        for y in 0..vis.height {
            for x in 0..vis.width {
                let mut m = 0i32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if vis.get_clamped(x + dx, y + dy) == 255 {
                            m = m.max(i32::from(vis.get_clamped(x + dx, y + dy)));
                        }
                    }
                }
                next[(x + y * vis.width) as usize] = m as u8;
            }
        }
        vis.values = next;
    }

    for _pass in 0..32 {
        let mut next = vis.values.clone();
        for y in 0..vis.height {
            for x in 0..vis.width {
                if vis.get_clamped(x, y) < 255 {
                    let mut m = 0;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if vis.get_clamped(x + dx, y + dy) == 255 {
                                m += 1;
                            }
                        }
                    }
                    if m >= 5 {
                        next[(x + y * vis.width) as usize] = 255;
                    }
                }
            }
        }
        vis.values = next;
    }

    let mut orig = vis.values.clone();

    let mut filled_pixels: Vec<(i32, i32)> = Vec::with_capacity(256);
    for by in 0..vis.height {
        for bx in 0..vis.width {
            let is_ultrasmooth = vis.values[(bx + by * vis.width) as usize] == 0;
            if !is_ultrasmooth {
                continue;
            }

            filled_pixels.clear();
            let total_set_pixels = vis.flood_fill(bx, by, &mut filled_pixels);

            if total_set_pixels < ULTRASMOOTH_REGION_TOO_SMALL_THRESHOLD {
                for &(x, y) in &filled_pixels {
                    orig[(x + y * vis.width) as usize] = 255;
                }
            }
        }
    }

    vis.values = orig;

    let mut block_mse_scales = vec![0.0f32; total_blocks as usize];

    let mut total_ultrasmooth_blocks = 0u32;
    for i in 0..total_blocks as usize {
        let is_ultrasmooth = vis.values[i] == 0;
        block_mse_scales[i] = if is_ultrasmooth {
            ULTRAMOOTH_BLOCK_MSE_SCALE
        } else {
            -1.0
        };
        total_ultrasmooth_blocks += u32::from(is_ultrasmooth);
    }

    if rdo_debug_output {
        println!(
            "Total ultrasmooth blocks: {:3.2}%",
            total_ultrasmooth_blocks as f32 * 100.0 / total_blocks as f32
        );
    }

    block_mse_scales
}

/// What the unpackers handed to the rate-distortion pass go by: `unpacker_funcs`.
#[derive(Clone, Copy)]
struct UnpackerFuncs {
    mode: Bc1ApproxMode,
    allow_3color_mode: bool,
    use_bc1_3color_mode_for_black: bool,
}

impl UnpackerFuncs {
    fn unpack_bc1_block(&self, block: &[u8], pixels: &mut [ColorRgba]) -> bool {
        let block: &[u8; 8] = block.try_into().expect("a BC1 block");
        let used_3color_mode = rgbcx::unpack_bc1(
            block,
            pixels.try_into().expect("sixteen pixels"),
            true,
            self.mode,
        );
        if used_3color_mode {
            if !self.allow_3color_mode {
                return false;
            }

            if !self.use_bc1_3color_mode_for_black {
                for y in 0..4 {
                    for x in 0..4 {
                        if (block[4 + y] >> (x * 2)) & 3 == 3 {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    /// (The crate's own, for BC2: the alpha half of a block, to red as BC4 goes.)
    fn unpack_bc2_alpha_block(block: &[u8], pixels: &mut [ColorRgba]) -> bool {
        pixels.fill([0; 4]);
        bc2::unpack_bc2_alpha(
            block.try_into().expect("the alpha of a BC2 block"),
            pixels.as_flattened_mut(),
            4,
        );
        true
    }

    // TODO: Enforce 6/8 color constraints
    fn unpack_bc4_block(block: &[u8], pixels: &mut [ColorRgba]) -> bool {
        pixels.fill([0; 4]);
        rgbcx::unpack_bc4(
            block.try_into().expect("a BC4 block"),
            pixels.as_flattened_mut(),
            4,
        );
        true
    }

    fn unpack_bc7_block(block: &[u8], pixels: &mut [ColorRgba]) -> bool {
        bc7decomp::unpack_bc7(
            block.try_into().expect("a BC7 block"),
            pixels.try_into().expect("sixteen pixels"),
        )
    }
}

/// The encoder of an image: `rdo_bc_encoder`. [`RdoBcEncoder::init`] it with an image and
/// settings, [`RdoBcEncoder::encode`], and take the [`RdoBcEncoder::blocks`].
pub struct RdoBcEncoder {
    source_image: Image,
    params: RdoBcParams,
    orig_width: u32,
    orig_height: u32,
    blocks_x: u32,
    blocks_y: u32,
    total_blocks: u32,
    bytes_per_block: u32,
    pixel_format_bpp: u32,
    total_texels: u32,
    // (The C++ looks for alpha in the image but never keeps what it finds, so this stays false
    // and bc7enc's settings for images with alpha are never used. Kept, as the blocks depend
    // on it.)
    has_alpha: bool,
    packed_image: Vec<u8>,
    prerdo_packed_image: Vec<u8>,
    total_modified: [u32; 2],
    bc7enc_pack_params: bc7enc::CompressBlockParams,
    bc7e_pack_params: bc7e::CompressBlockParams,
    rgbcx_encoder: Rgbcx,
    bc7enc_encoder: Bc7Enc,
    bc7e_encoder: Bc7e,
}

impl Default for RdoBcEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RdoBcEncoder {
    /// An encoder with nothing in it yet.
    pub fn new() -> Self {
        let params = RdoBcParams::new();
        RdoBcEncoder {
            source_image: Image::default(),
            params,
            orig_width: 0,
            orig_height: 0,
            blocks_x: 0,
            blocks_y: 0,
            total_blocks: 0,
            bytes_per_block: 0,
            pixel_format_bpp: 0,
            total_texels: 0,
            has_alpha: false,
            packed_image: Vec::new(),
            prerdo_packed_image: Vec::new(),
            total_modified: [0; 2],
            bc7enc_pack_params: bc7enc::CompressBlockParams::new(),
            bc7e_pack_params: bc7e::CompressBlockParams::new(false),
            // `init_encoders`, done again by `init` for the settings' BC1 mode.
            rgbcx_encoder: Rgbcx::new(params.bc1_mode),
            bc7enc_encoder: Bc7Enc::new(),
            bc7e_encoder: Bc7e::new(),
        }
    }

    /// Empties the encoder.
    pub fn clear(&mut self) {
        self.source_image = Image::default();
        self.params.clear();
        self.orig_width = 0;
        self.orig_height = 0;
        self.blocks_x = 0;
        self.blocks_y = 0;
        self.total_blocks = 0;
        self.bytes_per_block = 0;
        self.pixel_format_bpp = 0;
        self.total_texels = 0;
        self.has_alpha = false;
        self.packed_image.clear();
        self.prerdo_packed_image.clear();
        self.total_modified = [0; 2];
        self.bc7enc_pack_params.clear();
        self.bc7e_pack_params = bc7e::CompressBlockParams::new(false);
    }

    /// Takes an image (a copy of it) and the settings. The image is turned upside down if the
    /// settings say so and padded to whole blocks by repeating its last column and row.
    pub fn init(&mut self, src_image: &Image, params: &RdoBcParams) -> bool {
        self.clear();

        self.params = *params;

        // init_encoders
        if self.rgbcx_encoder.approx_mode() != self.params.bc1_mode {
            self.rgbcx_encoder = Rgbcx::new(self.params.bc1_mode);
        }

        self.init_source_image(src_image)
    }

    /// Packs the image, then runs the rate-distortion pass if lambda is above 0. False when
    /// there is no image or it has no blocks.
    pub fn encode(&mut self) -> bool {
        if self.packed_image.is_empty() {
            return false;
        }

        self.init_encoder_params();
        self.encode_texture();
        self.postprocess_rdo();

        true
    }

    /// The settings, as the encoder holds them (it switches `perceptual` off for BC7 with the
    /// rate-distortion pass).
    pub fn params(&self) -> &RdoBcParams {
        &self.params
    }

    /// The image as packed: turned and padded.
    pub fn source_image(&self) -> &Image {
        &self.source_image
    }

    /// The blocks as they were before the rate-distortion pass.
    pub fn prerdo_blocks(&self) -> &[u8] {
        &self.prerdo_packed_image
    }

    /// The packed blocks, in rows from the top.
    pub fn blocks(&self) -> &[u8] {
        &self.packed_image
    }

    /// The format packed.
    pub fn format(&self) -> BlockFormat {
        self.params.format
    }

    /// The width of the image given, before padding.
    pub fn orig_width(&self) -> u32 {
        self.orig_width
    }

    /// The height of the image given, before padding.
    pub fn orig_height(&self) -> u32 {
        self.orig_height
    }

    pub fn blocks_x(&self) -> u32 {
        self.blocks_x
    }

    pub fn blocks_y(&self) -> u32 {
        self.blocks_y
    }

    pub fn total_blocks(&self) -> u32 {
        self.total_blocks
    }

    pub fn total_blocks_size_in_bytes(&self) -> u32 {
        self.total_blocks * self.bytes_per_block
    }

    pub fn bytes_per_block(&self) -> u32 {
        self.bytes_per_block
    }

    /// Bits for each pixel: 4 or 8.
    pub fn pixel_format_bpp(&self) -> u32 {
        self.pixel_format_bpp
    }

    pub fn total_texels(&self) -> u32 {
        self.total_texels
    }

    /// How many blocks the rate-distortion pass changed. For BC3 the colour halves and then the
    /// alpha halves; for BC5 the first channel's halves and then the second's; for the others
    /// the blocks, and 0.
    pub fn total_modified(&self) -> [u32; 2] {
        self.total_modified
    }

    /// For BC7, how many of the blocks are of each mode.
    pub fn bc7_mode_histogram(&self) -> [u32; 8] {
        let mut hist = [0u32; 8];
        if self.params.format == BlockFormat::Bc7 {
            for block in self.packed_image.as_chunks::<16>().0.iter() {
                let mode = block[0].trailing_zeros();
                if mode < 8 {
                    hist[mode as usize] += 1;
                }
            }
        }
        hist
    }

    fn init_encoder_params(&mut self) {
        self.bc7enc_pack_params = bc7enc::CompressBlockParams::new();
        if !self.params.perceptual {
            self.bc7enc_pack_params.init_linear_weights();
        }
        self.bc7enc_pack_params.max_partitions = self.params.bc7enc_max_partitions_to_scan;
        self.bc7enc_pack_params.uber_level = bc7enc::MAX_UBER_LEVEL.min(self.params.bc7_uber_level);

        if self.params.bc7enc_mode6_only {
            self.bc7enc_pack_params.mode_mask = 1 << 6;
        }

        if self.params.format == BlockFormat::Bc7 && self.params.rdo_lambda > 0.0 {
            // Slam off perceptual in RDO mode - we don't support it (too slow).
            self.params.perceptual = false;
            self.bc7enc_pack_params.perceptual = false;
            self.bc7enc_pack_params.init_linear_weights();
        }

        if self.params.format == BlockFormat::Bc7 && self.params.bc7enc_reduce_entropy {
            // Configure the BC7 encoder with some decent parameters for later RDO post-processing.
            // Textures with alpha are harder for BC7 to
            // handle, so we use more conservative defaults.

            self.bc7enc_pack_params
                .mode17_partition_estimation_filterbank = false;

            if self.params.bc7enc_rdo_bc7_weight_modes {
                // Weight modes 5 and especially 6 more highly than the other modes.
                if self.has_alpha {
                    self.bc7enc_pack_params.mode5_error_weight = 0.7;
                    self.bc7enc_pack_params.mode6_error_weight = 0.6;
                } else {
                    self.bc7enc_pack_params.mode6_error_weight = 0.4;
                }
            }

            if self.params.bc7enc_rdo_bc7_weight_low_frequency_partitions {
                // Slightly prefer the lower frequency partition patterns.
                self.bc7enc_pack_params.low_frequency_partition_weight = 0.9999;
            }

            if self.params.bc7enc_rdo_bc7_quant_mode6_endpoints {
                // As a good default, don't quantize mode 6 endpoints if the
                // texture has alpha. This isn't required, but helps mask textures.
                self.bc7enc_pack_params.quant_mode6_endpoints = true;
            }

            if self.params.bc7enc_rdo_bc7_pbit1_weighting {
                // Favor p-bit 0 vs. 1, to slightly lower the entropy of output blocks with p-bits
                self.bc7enc_pack_params.pbit1_weight = 1.3;
            }
        }

        // Now initialize the BC7 compressor's parameters.
        let perceptual = self.params.perceptual;
        self.bc7e_pack_params = match self.params.bc7_uber_level {
            0 => bc7e::CompressBlockParams::ultrafast(perceptual),
            1 => bc7e::CompressBlockParams::veryfast(perceptual),
            2 => bc7e::CompressBlockParams::fast(perceptual),
            3 => bc7e::CompressBlockParams::basic(perceptual),
            4 => bc7e::CompressBlockParams::slow(perceptual),
            5 => bc7e::CompressBlockParams::veryslow(perceptual),
            _ => bc7e::CompressBlockParams::slowest(perceptual),
        };
    }

    fn init_source_image(&mut self, orig_source_image: &Image) -> bool {
        self.pixel_format_bpp = self.params.format.bytes_per_block() / 2;
        self.bytes_per_block = (16 * self.pixel_format_bpp) / 8;

        self.orig_width = orig_source_image.width;
        self.orig_height = orig_source_image.height;

        self.source_image = if self.params.y_flip {
            orig_source_image.flipped_y()
        } else {
            orig_source_image.clone()
        };

        self.source_image.crop_dup_borders(
            (self.source_image.width + 3) & !3,
            (self.source_image.height + 3) & !3,
        );

        self.blocks_x = self.source_image.width / 4;
        self.blocks_y = self.source_image.height / 4;
        self.total_blocks = self.blocks_x * self.blocks_y;
        self.total_texels = self.total_blocks * 16;

        self.packed_image = vec![0u8; self.total_blocks as usize * self.bytes_per_block as usize];

        true
    }

    fn encode_texture(&mut self) {
        let row_bytes = self.blocks_x as usize * self.bytes_per_block as usize;
        if row_bytes == 0 {
            return;
        }
        let threads = parallel::worker_count(self.params.threads);
        let mut packed_image = std::mem::take(&mut self.packed_image);
        let this = &*self;
        let rows: Vec<(u32, &mut [u8])> = packed_image
            .chunks_exact_mut(row_bytes)
            .enumerate()
            .map(|(by, row)| (by as u32, row))
            .collect();
        parallel::for_each(rows, threads, |(by, row)| this.encode_row(by, row));
        self.packed_image = packed_image;
    }

    /// Packs a row of blocks.
    fn encode_row(&self, by: u32, row: &mut [u8]) {
        let params = &self.params;
        for (bx, block) in row
            .chunks_exact_mut(self.bytes_per_block as usize)
            .enumerate()
        {
            let pixels = self.source_image.get_block(bx as u32, by);

            match params.format {
                BlockFormat::Bc1 => {
                    self.rgbcx_encoder.encode_bc1(
                        params.bc1_quality_level,
                        block.try_into().expect("8 bytes"),
                        &pixels,
                        params.use_bc1_3color_mode,
                        params.use_bc1_3color_mode_for_black,
                        None,
                    );
                }
                BlockFormat::Bc2 => {
                    bc2::encode_bc2(
                        &self.rgbcx_encoder,
                        params.bc1_quality_level,
                        block.try_into().expect("16 bytes"),
                        &pixels,
                    );
                }
                BlockFormat::Bc3 => {
                    let block: &mut [u8; 16] = block.try_into().expect("16 bytes");
                    if params.use_hq_bc345 {
                        self.rgbcx_encoder.encode_bc3_hq(
                            params.bc1_quality_level,
                            block,
                            &pixels,
                            params.bc345_search_rad,
                            params.bc345_mode_mask,
                        );
                    } else {
                        self.rgbcx_encoder
                            .encode_bc3(params.bc1_quality_level, block, &pixels);
                    }
                }
                BlockFormat::Bc4 => {
                    let block: &mut [u8; 8] = block.try_into().expect("8 bytes");
                    let channel = &pixels.as_flattened()[params.bc45_channel0 as usize..];
                    if params.use_hq_bc345 {
                        rgbcx::encode_bc4_hq(
                            block,
                            channel,
                            4,
                            params.bc345_search_rad,
                            params.bc345_mode_mask,
                            None,
                        );
                    } else {
                        rgbcx::encode_bc4(block, channel, 4);
                    }
                }
                BlockFormat::Bc5 => {
                    let block: &mut [u8; 16] = block.try_into().expect("16 bytes");
                    if params.use_hq_bc345 {
                        rgbcx::encode_bc5_hq(
                            block,
                            pixels.as_flattened(),
                            params.bc45_channel0 as usize,
                            params.bc45_channel1 as usize,
                            4,
                            params.bc345_search_rad,
                            params.bc345_mode_mask,
                        );
                    } else {
                        rgbcx::encode_bc5(
                            block,
                            pixels.as_flattened(),
                            params.bc45_channel0 as usize,
                            params.bc45_channel1 as usize,
                            4,
                        );
                    }
                }
                BlockFormat::Bc7 => {
                    let block: &mut [u8; 16] = block.try_into().expect("16 bytes");
                    if params.use_bc7e {
                        self.bc7e_encoder
                            .compress_block(block, &pixels, &self.bc7e_pack_params);
                    } else {
                        self.bc7enc_encoder.compress_block(
                            block,
                            &pixels,
                            &self.bc7enc_pack_params,
                        );
                    }
                }
            }
        }
    }

    fn postprocess_rdo(&mut self) {
        self.prerdo_packed_image = self.packed_image.clone();
        self.total_modified = [0; 2];

        // Post-process the data with Rate Distortion Optimization
        if self.params.rdo_lambda <= 0.0 {
            return;
        }

        let params = self.params;
        let total_blocks = self.total_blocks;

        const MIN_RDO_MULTITHREADING_BLOCKS: u32 = 4096;
        let rdo_total_threads = if params.rdo_multithreading
            && params.rdo_max_threads > 1
            && total_blocks >= MIN_RDO_MULTITHREADING_BLOCKS
        {
            params.rdo_max_threads
        } else {
            1
        };

        // The strips: where each starts and how many blocks it has.
        let mut strips: Vec<(usize, usize)> = Vec::with_capacity(rdo_total_threads as usize);
        let (mut blocks_remaining, mut cur_block_index) = (total_blocks, 0u32);
        for p in 0..rdo_total_threads {
            let num_blocks = if p == rdo_total_threads - 1 {
                blocks_remaining
            } else {
                total_blocks / rdo_total_threads
            };
            strips.push((cur_block_index as usize, num_blocks as usize));
            cur_block_index += num_blocks;
            blocks_remaining -= num_blocks;
        }

        let mut ert_p = ReduceEntropyParams::new();

        ert_p.lambda = params.rdo_lambda;
        ert_p.lookback_window_size = params.lookback_window_size;
        ert_p.smooth_block_max_mse_scale = params.rdo_smooth_block_error_scale;
        ert_p.max_smooth_block_std_dev = params.rdo_max_smooth_block_std_dev;
        ert_p.debug_output = params.rdo_debug_output;
        ert_p.try_two_matches = params.rdo_try_2_matches;
        ert_p.allow_relative_movement = params.rdo_allow_relative_movement;
        ert_p.skip_zero_mse_blocks = false;

        let mut block_rgb_mse_scales = compute_block_mse_scales(
            &self.source_image,
            self.blocks_x,
            self.blocks_y,
            total_blocks,
            params.rdo_debug_output,
        );

        let mut block_pixels: Vec<ColorRgba> = Vec::with_capacity(total_blocks as usize * 16);
        for by in 0..self.blocks_y {
            for bx in 0..self.blocks_x {
                block_pixels.extend_from_slice(&self.source_image.get_block(bx, by));
            }
        }

        let mut block_unpackers = UnpackerFuncs {
            allow_3color_mode: params.use_bc1_3color_mode,
            use_bc1_3color_mode_for_black: params.use_bc1_3color_mode_for_black,
            mode: params.bc1_mode,
        };

        // (For the formats that scale the error of very smooth blocks.)
        let scale_rgb_mse_scales = |scales: &mut [f32], ert_p: &ReduceEntropyParams| {
            for s in scales.iter_mut() {
                if *s > 0.0 {
                    *s = max_f(
                        ert_p.smooth_block_max_mse_scale,
                        *s * min_f(ert_p.lambda, 3.0),
                    );
                }
            }
        };

        // What each strip does: the passes over it, each with its settings, where in a block it
        // starts and how many bytes it goes over, the channels, the pixels, the unpacker, and
        // whether it gets the smooth-block scales.
        struct Pass<'a> {
            offset: usize,
            size: u32,
            num_comps: u32,
            pixels: &'a [ColorRgba],
            params: ReduceEntropyParams,
            unpacker: Unpacker,
            use_scales: bool,
            counter: usize,
        }
        #[derive(Clone, Copy)]
        enum Unpacker {
            Bc1,
            Bc2Alpha,
            Bc4,
            Bc7,
        }

        let single_channel = |channel: usize| -> Vec<ColorRgba> {
            block_pixels
                .iter()
                .map(|pixel| [pixel[channel], 0, 0, 0])
                .collect()
        };
        let (block_pixels_first, block_pixels_second);

        let stride = self.bytes_per_block;
        let passes: Vec<Pass> = match params.format {
            BlockFormat::Bc7 => {
                ert_p.lookback_window_size = params.lookback_window_size.max(16);

                // BC7 RDO
                if !params.custom_rdo_smooth_block_error_scale {
                    // Attempt to compute a decent conservative smooth block MSE max scaling factor.
                    // No single smooth block scale setting can work for all textures
                    // (unless it's ridiuclously large, killing efficiency).
                    ert_p.smooth_block_max_mse_scale =
                        lerp(15.0, 50.0, min_f(1.0, ert_p.lambda / 4.0));
                }

                scale_rgb_mse_scales(&mut block_rgb_mse_scales, &ert_p);

                vec![Pass {
                    offset: 0,
                    size: 16,
                    num_comps: 4,
                    pixels: &block_pixels,
                    params: ert_p,
                    unpacker: Unpacker::Bc7,
                    use_scales: true,
                    counter: 0,
                }]
            }
            BlockFormat::Bc5 => {
                // BC5 RDO - One BC4 block for R followed by one BC4 block for G
                ert_p.lookback_window_size = params.lookback_window_size.max(16);

                // (Red and green, whatever the two channels packed are.)
                block_pixels_first = single_channel(0);
                block_pixels_second = single_channel(1);

                ert_p.color_weights[1] = 0;
                ert_p.color_weights[2] = 0;
                ert_p.color_weights[3] = 0;

                if !params.custom_rdo_smooth_block_error_scale {
                    ert_p.smooth_block_max_mse_scale =
                        lerp(10.0, 30.0, min_f(1.0, ert_p.lambda / 4.0));
                }

                vec![
                    Pass {
                        offset: 0,
                        size: 8,
                        num_comps: 1,
                        pixels: &block_pixels_first,
                        params: ert_p,
                        unpacker: Unpacker::Bc4,
                        use_scales: false,
                        counter: 0,
                    },
                    Pass {
                        offset: 8,
                        size: 8,
                        num_comps: 1,
                        pixels: &block_pixels_second,
                        params: ert_p,
                        unpacker: Unpacker::Bc4,
                        use_scales: false,
                        counter: 1,
                    },
                ]
            }
            BlockFormat::Bc4 => {
                // BC4 RDO - One BC4 block for R
                ert_p.color_weights[1] = 0;
                ert_p.color_weights[2] = 0;
                ert_p.color_weights[3] = 0;

                if !params.custom_rdo_smooth_block_error_scale {
                    ert_p.smooth_block_max_mse_scale =
                        lerp(10.0, 30.0, min_f(1.0, ert_p.lambda / 4.0));
                }

                // (Against the red of the pixels, whatever the channel packed is.)
                vec![Pass {
                    offset: 0,
                    size: 8,
                    num_comps: 1,
                    pixels: &block_pixels,
                    params: ert_p,
                    unpacker: Unpacker::Bc4,
                    use_scales: false,
                    counter: 0,
                }]
            }
            BlockFormat::Bc1 => {
                // BC1 RDO - One BC1 block
                ert_p.color_weights[3] = 0;

                if !params.custom_rdo_smooth_block_error_scale {
                    // This is just a hack - no single setting can work for all textures.
                    ert_p.smooth_block_max_mse_scale =
                        lerp(15.0, 50.0, min_f(1.0, ert_p.lambda / 8.0));
                }

                scale_rgb_mse_scales(&mut block_rgb_mse_scales, &ert_p);

                vec![Pass {
                    offset: 0,
                    size: 8,
                    num_comps: 3,
                    pixels: &block_pixels,
                    params: ert_p,
                    unpacker: Unpacker::Bc1,
                    use_scales: true,
                    counter: 0,
                }]
            }
            BlockFormat::Bc2 | BlockFormat::Bc3 => {
                // BC3 RDO - One BC4 block followed by one BC1 block
                // (BC2, the crate's own, goes the same way: its sixteen alphas of four bits
                // are where BC3 has the BC4 block.)
                ert_p.lookback_window_size = params.lookback_window_size.max(16);

                block_pixels_first = single_channel(3);

                ert_p.color_weights[3] = 0;

                let mut ert_alpha_p = ert_p;
                ert_alpha_p.color_weights[1] = 0;
                ert_alpha_p.color_weights[2] = 0;
                ert_alpha_p.color_weights[3] = 0;

                if !params.custom_rdo_smooth_block_error_scale {
                    // This is just a hack - no single setting can work for all textures.
                    ert_p.smooth_block_max_mse_scale =
                        lerp(15.0, 50.0, min_f(1.0, ert_p.lambda / 8.0));

                    ert_alpha_p.smooth_block_max_mse_scale =
                        lerp(10.0, 30.0, min_f(1.0, ert_alpha_p.lambda / 4.0));
                }

                scale_rgb_mse_scales(&mut block_rgb_mse_scales, &ert_p);

                block_unpackers.allow_3color_mode = false;
                block_unpackers.use_bc1_3color_mode_for_black = false;

                vec![
                    Pass {
                        offset: 0,
                        size: 8,
                        num_comps: 1,
                        pixels: &block_pixels_first,
                        params: ert_alpha_p,
                        unpacker: if params.format == BlockFormat::Bc2 {
                            Unpacker::Bc2Alpha
                        } else {
                            Unpacker::Bc4
                        },
                        use_scales: false,
                        counter: 1,
                    },
                    Pass {
                        offset: 8,
                        size: 8,
                        num_comps: 3,
                        pixels: &block_pixels,
                        params: ert_p,
                        unpacker: Unpacker::Bc1,
                        use_scales: true,
                        counter: 0,
                    },
                ]
            }
        };

        let total_modified = [AtomicU32::new(0), AtomicU32::new(0)];
        let use_scales = params.rdo_ultrasmooth_block_handling;
        let block_rgb_mse_scales = &block_rgb_mse_scales;
        let passes = &passes;
        let total_modified_ref = &total_modified;

        // Each strip with its part of the blocks.
        let mut jobs: Vec<(usize, usize, &mut [u8])> = Vec::with_capacity(strips.len());
        let mut rest = &mut self.packed_image[..];
        for &(first_block, num_blocks) in &strips {
            let (strip, after) = rest.split_at_mut(num_blocks * stride as usize);
            rest = after;
            if num_blocks != 0 {
                jobs.push((first_block, num_blocks, strip));
            }
        }

        parallel::for_each(
            jobs,
            parallel::worker_count(params.threads),
            |(first_block, num_blocks, strip)| {
                for pass in passes {
                    let mut total_modified_local = 0u32;
                    let pixels = &pass.pixels[16 * first_block..16 * (first_block + num_blocks)];
                    let scales = if pass.use_scales && use_scales {
                        Some(&block_rgb_mse_scales[first_block..first_block + num_blocks])
                    } else {
                        None
                    };
                    let blocks = &mut strip[pass.offset..];
                    match pass.unpacker {
                        Unpacker::Bc1 => ert::reduce_entropy(
                            blocks,
                            num_blocks as u32,
                            stride,
                            pass.size,
                            4,
                            4,
                            pass.num_comps,
                            pixels,
                            &pass.params,
                            &mut total_modified_local,
                            |block, pixels, _| block_unpackers.unpack_bc1_block(block, pixels),
                            scales,
                        ),
                        Unpacker::Bc2Alpha => ert::reduce_entropy(
                            blocks,
                            num_blocks as u32,
                            stride,
                            pass.size,
                            4,
                            4,
                            pass.num_comps,
                            pixels,
                            &pass.params,
                            &mut total_modified_local,
                            |block, pixels, _| UnpackerFuncs::unpack_bc2_alpha_block(block, pixels),
                            scales,
                        ),
                        Unpacker::Bc4 => ert::reduce_entropy(
                            blocks,
                            num_blocks as u32,
                            stride,
                            pass.size,
                            4,
                            4,
                            pass.num_comps,
                            pixels,
                            &pass.params,
                            &mut total_modified_local,
                            |block, pixels, _| UnpackerFuncs::unpack_bc4_block(block, pixels),
                            scales,
                        ),
                        Unpacker::Bc7 => ert::reduce_entropy(
                            blocks,
                            num_blocks as u32,
                            stride,
                            pass.size,
                            4,
                            4,
                            pass.num_comps,
                            pixels,
                            &pass.params,
                            &mut total_modified_local,
                            |block, pixels, _| UnpackerFuncs::unpack_bc7_block(block, pixels),
                            scales,
                        ),
                    };
                    total_modified_ref[pass.counter]
                        .fetch_add(total_modified_local, Ordering::Relaxed);
                }
            },
        );

        self.total_modified = [
            total_modified[0].load(Ordering::Relaxed),
            total_modified[1].load(Ordering::Relaxed),
        ];
    }

    /// Unpacks the blocks to an image of whole blocks. None when a BC7 block is of no mode,
    /// which the encoders do not make.
    pub fn unpack_blocks(&self) -> Option<Image> {
        let mut unpacked_image = Image::new(self.blocks_x * 4, self.blocks_y * 4);
        if self.total_blocks == 0 {
            return Some(unpacked_image);
        }

        let row_pixels = self.blocks_x as usize * 4 * 4;
        let row_bytes = self.blocks_x as usize * self.bytes_per_block as usize;
        let unpack_failed = AtomicU32::new(0);
        let format = self.params.format;
        let mode = self.params.bc1_mode;
        let bytes_per_block = self.bytes_per_block as usize;

        let rows: Vec<(&[u8], &mut [ColorRgba])> = self
            .packed_image
            .chunks_exact(row_bytes)
            .zip(unpacked_image.pixels.chunks_exact_mut(row_pixels))
            .collect();
        parallel::for_each(
            rows,
            parallel::worker_count(self.params.threads),
            |(blocks, pixels)| {
                for (bx, block) in blocks.chunks_exact(bytes_per_block).enumerate() {
                    let mut unpacked_pixels = [[0u8, 0, 0, 255]; 16];

                    match format {
                        BlockFormat::Bc1 => {
                            rgbcx::unpack_bc1(
                                block.try_into().expect("8 bytes"),
                                &mut unpacked_pixels,
                                true,
                                mode,
                            );
                        }
                        BlockFormat::Bc2 => {
                            bc2::unpack_bc2(
                                block.try_into().expect("16 bytes"),
                                &mut unpacked_pixels,
                                mode,
                            );
                        }
                        BlockFormat::Bc3 => {
                            rgbcx::unpack_bc3(
                                block.try_into().expect("16 bytes"),
                                &mut unpacked_pixels,
                                mode,
                            );
                        }
                        BlockFormat::Bc4 => {
                            rgbcx::unpack_bc4(
                                block.try_into().expect("8 bytes"),
                                unpacked_pixels.as_flattened_mut(),
                                4,
                            );
                        }
                        BlockFormat::Bc5 => {
                            rgbcx::unpack_bc5(
                                block.try_into().expect("16 bytes"),
                                unpacked_pixels.as_flattened_mut(),
                                0,
                                1,
                                4,
                            );
                        }
                        BlockFormat::Bc7 => {
                            if !bc7decomp::unpack_bc7(
                                block.try_into().expect("16 bytes"),
                                &mut unpacked_pixels,
                            ) {
                                unpack_failed.store(1, Ordering::Relaxed);
                            }
                        }
                    }

                    // set_block
                    let width = row_pixels / 4;
                    for y in 0..4 {
                        pixels[y * width + bx * 4..y * width + bx * 4 + 4]
                            .copy_from_slice(&unpacked_pixels[y * 4..y * 4 + 4]);
                    }
                }
            },
        );

        (unpack_failed.load(Ordering::Relaxed) == 0).then_some(unpacked_image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 70 by 50 image: a ramp with a block of noise in it, and alpha that varies.
    fn image() -> Image {
        let mut image = Image::new(70, 50);
        let mut state = 12345u32;
        for y in 0..50u32 {
            for x in 0..70u32 {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let noisy = (20..40).contains(&x) && (10..30).contains(&y);
                let value = if noisy {
                    (state >> 24) as u8
                } else {
                    (x * 3) as u8
                };
                image.pixels[(x + y * 70) as usize] =
                    [value, (y * 5) as u8, 128, 255 - (x + y) as u8];
            }
        }
        image
    }

    fn encode(params: &RdoBcParams) -> RdoBcEncoder {
        let mut encoder = RdoBcEncoder::new();
        assert!(encoder.init(&image(), params));
        assert!(encoder.encode());
        encoder
    }

    #[test]
    fn the_image_is_padded_to_whole_blocks() {
        let mut params = RdoBcParams::new();
        params.format = BlockFormat::Bc1;
        params.bc1_quality_level = 2;
        let encoder = encode(&params);
        assert_eq!((encoder.orig_width(), encoder.orig_height()), (70, 50));
        assert_eq!(
            (
                encoder.blocks_x(),
                encoder.blocks_y(),
                encoder.total_blocks()
            ),
            (18, 13, 234)
        );
        assert_eq!(encoder.blocks().len(), 234 * 8);
        assert_eq!(encoder.pixel_format_bpp(), 4);
        // The padding repeats the last column and row.
        let source = encoder.source_image();
        assert_eq!((source.width, source.height), (72, 52));
        assert_eq!(source.pixel(71, 51), source.pixel(69, 49));
        // Without the pass, the blocks before it are the blocks.
        assert_eq!(encoder.blocks(), encoder.prerdo_blocks());
        assert_eq!(encoder.total_modified(), [0, 0]);
        let unpacked = encoder.unpack_blocks().unwrap();
        assert_eq!((unpacked.width, unpacked.height), (72, 52));
    }

    #[test]
    fn every_format_packs_and_unpacks_close_to_the_image() {
        for format in [
            BlockFormat::Bc1,
            BlockFormat::Bc2,
            BlockFormat::Bc3,
            BlockFormat::Bc4,
            BlockFormat::Bc5,
            BlockFormat::Bc7,
        ] {
            let mut params = RdoBcParams::new();
            params.format = format;
            params.bc1_quality_level = 5;
            params.bc7_uber_level = 1;
            let encoder = encode(&params);
            assert_eq!(
                encoder.blocks().len() as u32,
                encoder.total_blocks() * format.bytes_per_block()
            );
            let unpacked = encoder.unpack_blocks().unwrap();
            // The channels a format holds come back near the source, away from the noise.
            let channels: &[usize] = match format {
                BlockFormat::Bc1 => &[0, 1, 2],
                BlockFormat::Bc2 | BlockFormat::Bc3 | BlockFormat::Bc7 => &[0, 1, 2, 3],
                BlockFormat::Bc4 => &[0],
                BlockFormat::Bc5 => &[0, 1],
            };
            let (source, got) = (encoder.source_image().pixel(50, 40), unpacked.pixel(50, 40));
            for &c in channels {
                assert!(
                    (i32::from(source[c]) - i32::from(got[c])).abs() <= 12,
                    "{format:?} channel {c}: {source:?} {got:?}"
                );
            }
        }
    }

    #[test]
    fn the_number_of_threads_does_not_change_the_blocks() {
        let mut params = RdoBcParams::new();
        params.format = BlockFormat::Bc7;
        params.bc7_uber_level = 0;
        params.rdo_lambda = 2.0;
        params.threads = 1;
        let one = encode(&params);
        assert!(one.total_modified()[0] > 0);
        assert_ne!(one.blocks(), one.prerdo_blocks());
        for threads in [0, 2, 7] {
            params.threads = threads;
            let other = encode(&params);
            assert_eq!(one.blocks(), other.blocks(), "{threads} threads");
            assert_eq!(one.total_modified(), other.total_modified());
        }
        assert_eq!(
            one.bc7_mode_histogram().iter().sum::<u32>(),
            one.total_blocks()
        );
    }

    #[test]
    fn bc2_goes_through_the_pass_as_bc3_does() {
        let mut params = RdoBcParams::new();
        params.format = BlockFormat::Bc2;
        params.bc1_quality_level = 5;
        params.rdo_lambda = 3.0;
        params.threads = 1;
        let encoder = encode(&params);
        // Colours and alphas were both changed, and the colours are still of four.
        assert!(
            encoder.total_modified()[0] > 0 && encoder.total_modified()[1] > 0,
            "{:?}",
            encoder.total_modified()
        );
        assert_ne!(encoder.blocks(), encoder.prerdo_blocks());
        assert!(encoder
            .blocks()
            .as_chunks::<16>()
            .0
            .iter()
            .all(|block| u16::from_le_bytes([block[8], block[9]])
                > u16::from_le_bytes([block[10], block[11]])));
        let unpacked = encoder.unpack_blocks().unwrap();
        let (source, got) = (encoder.source_image().pixel(60, 44), unpacked.pixel(60, 44));
        for c in 0..4 {
            assert!(
                (i32::from(source[c]) - i32::from(got[c])).abs() <= 24,
                "channel {c}: {source:?} {got:?}"
            );
        }
        params.threads = 0;
        assert_eq!(encode(&params).blocks(), encoder.blocks());
    }

    #[test]
    fn perceptual_is_switched_off_for_bc7_with_the_pass() {
        let mut params = RdoBcParams::new();
        params.perceptual = true;
        params.bc7_uber_level = 0;
        params.rdo_lambda = 1.0;
        assert!(!encode(&params).params().perceptual);
        params.rdo_lambda = 0.0;
        assert!(encode(&params).params().perceptual);
    }

    #[test]
    fn an_image_of_nothing_is_refused() {
        let mut encoder = RdoBcEncoder::new();
        assert!(!encoder.encode());
        assert!(encoder.init(&Image::new(0, 0), &RdoBcParams::new()));
        assert!(!encoder.encode());
        assert_eq!(encoder.unpack_blocks().unwrap().pixels.len(), 0);
    }

    #[test]
    fn a_region_is_filled_and_counted() {
        // A 5 by 4 mask with a region of six, one of two, and walls between.
        #[rustfmt::skip]
        let values = vec![
            0, 0, 9, 0, 0,
            0, 9, 9, 9, 9,
            0, 0, 9, 9, 9,
            9, 0, 9, 9, 9,
        ];
        let mut mask = BlockMask {
            width: 5,
            height: 4,
            values,
        };
        let mut filled = Vec::new();
        assert_eq!(mask.flood_fill(0, 0, &mut filled), 6);
        assert_eq!(filled.len(), 6);
        assert_eq!(mask.values.iter().filter(|&&value| value == 255).count(), 6);
        assert_eq!(mask.flood_fill(0, 0, &mut filled), 0);
        assert_eq!(mask.flood_fill(4, 0, &mut filled), 2);
        assert!(!mask.values.contains(&0));
    }
}
