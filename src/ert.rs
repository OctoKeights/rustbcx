//! The entropy reduction transform, `bc7enc_rdo`'s rate-distortion pass: the port of `ert.cpp` and
//! `ert.h` (Richard Geldreich, Jr.).
//!
//! It goes over blocks that are already packed and, for each, tries copying runs of bytes into
//! it from the blocks just before. A copy that an LZ compressor (Deflate, LZMA, zstd) would
//! find as a match makes the texture smaller once it is compressed on disk; it also changes
//! what the block unpacks to. A copy is kept when the error it adds, weighed against the bits
//! it should save by [`ReduceEntropyParams::lambda`], comes out ahead.
//!
//! It works on any block format: the caller hands over [`reduce_entropy`] a function that
//! unpacks a block, and the pixels the blocks were packed from.

use crate::bc7decomp::ColorRgba;

const MAX_BLOCK_PIXELS: usize = 12 * 12;
const MAX_BLOCK_SIZE_IN_BYTES: usize = 256;
const MIN_MATCH_LEN: u32 = 3;
const LITERAL_BITS: f32 = 13.0;
const MATCH_CONTINUE_BITS: f32 = 1.0;
const MATCH_REP0_BITS: f32 = 4.0;

/// The settings of [`reduce_entropy`]: `reduce_entropy_params`.
// (The original's switches, each independent of the others.)
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReduceEntropyParams {
    /// The pass tries to reduce distortion x smooth block scale + rate x lambda (the rate is
    /// roughly LZ bits, the distortion the mean squared error). Larger values push it towards
    /// a lower rate, smaller ones towards less distortion. 0 is minimal distortion.
    pub lambda: f32,
    /// The number of bytes the pass can look back from each block to find matches. The larger,
    /// the slower, and the higher the quality for each LZ compressed bit.
    pub lookback_window_size: u32,
    /// How much the RMS error of a block may grow before a trial is rejected. 1.0 allows no
    /// increase, 1.05 allows 5%, and so on.
    pub max_allowed_rms_increase_ratio: f32,
    /// A block whose largest standard deviation over its channels is under this counts as
    /// smooth, the more so the further under.
    pub max_smooth_block_std_dev: f32,
    /// The most a smooth block's error is scaled up by, so that smooth blocks are changed less.
    pub smooth_block_max_mse_scale: f32,
    /// The weights of the four channels in the error. Those past the number of channels in use
    /// have to be 0.
    pub color_weights: [u32; 4],
    /// Try a second copy into a block that has taken one.
    pub try_two_matches: bool,
    /// Let a copy land at a different offset in the block than it came from. Much slower.
    pub allow_relative_movement: bool,
    /// Leave alone the blocks that unpack to exactly their pixels.
    pub skip_zero_mse_blocks: bool,
    /// Print statistics to standard output when done, as the C++ does.
    pub debug_output: bool,
}

impl Default for ReduceEntropyParams {
    fn default() -> Self {
        Self::new()
    }
}

impl ReduceEntropyParams {
    /// The C++'s defaults.
    #[must_use]
    pub fn new() -> Self {
        ReduceEntropyParams {
            lookback_window_size: 256,
            lambda: 1.0,
            max_allowed_rms_increase_ratio: 10.0,
            max_smooth_block_std_dev: 18.0,
            smooth_block_max_mse_scale: 10.0,
            color_weights: [1, 1, 1, 1],
            try_two_matches: false,
            allow_relative_movement: false,
            skip_zero_mse_blocks: false,
            debug_output: false,
        }
    }

    /// Back to the defaults.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl std::fmt::Display for ReduceEntropyParams {
    /// What the C++'s `print` prints.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "lambda: {:.6}", self.lambda)?;
        writeln!(f, "Lookback window size: {}", self.lookback_window_size)?;
        writeln!(
            f,
            "Max allowed RMS increase ratio: {:.6}",
            self.max_allowed_rms_increase_ratio
        )?;
        writeln!(
            f,
            "Max smooth block std dev: {:.6}",
            self.max_smooth_block_std_dev
        )?;
        writeln!(
            f,
            "Smooth block max MSE scale: {:.6}",
            self.smooth_block_max_mse_scale
        )?;
        writeln!(
            f,
            "Color weights: {} {} {} {}",
            self.color_weights[0],
            self.color_weights[1],
            self.color_weights[2],
            self.color_weights[3]
        )?;
        writeln!(f, "Try two matches: {}", u32::from(self.try_two_matches))?;
        writeln!(
            f,
            "Allow relative movement: {}",
            u32::from(self.allow_relative_movement)
        )?;
        writeln!(
            f,
            "Skip zero MSE blocks: {}",
            u32::from(self.skip_zero_mse_blocks)
        )
    }
}

fn clampf(mut value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        value = low;
    } else if value > high {
        value = high;
    }
    value
}

fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a + (b - a) * s
}

#[rustfmt::skip]
static TDEFL_SMALL_DIST_EXTRA: [u8; 512] = [
    0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7,
];

#[rustfmt::skip]
static TDEFL_LARGE_DIST_EXTRA: [u8; 128] = [
    0, 0, 8, 8, 9, 9, 9, 9, 10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
];

/// Roughly the bits Deflate spends on a match of this length at this distance.
fn compute_match_cost_estimate(mut dist: u32, match_len_in_bytes: u32) -> u32 {
    debug_assert!(match_len_in_bytes <= 258);

    let mut len_cost = 6u32;
    if match_len_in_bytes >= 12 {
        len_cost = 9;
    } else if match_len_in_bytes >= 8 {
        len_cost = 8;
    } else if match_len_in_bytes >= 6 {
        len_cost = 7;
    }

    let mut dist_cost = 5u32;
    if dist < 512 {
        dist_cost += u32::from(TDEFL_SMALL_DIST_EXTRA[(dist & 511) as usize]);
    } else {
        dist_cost += u32::from(TDEFL_LARGE_DIST_EXTRA[(dist.min(32767) >> 8) as usize]);
        while dist >= 32768 {
            dist_cost += 1;
            dist >>= 1;
        }
    }
    len_cost + dist_cost
}

/// `tracked_stat`: a count, a sum and a sum of squares.
#[derive(Clone, Copy, Default)]
struct TrackedStat {
    num: u32,
    total: u64,
    total2: u64,
}

impl TrackedStat {
    fn update(&mut self, val: u32) {
        self.num += 1;
        self.total += u64::from(val);
        self.total2 += u64::from(val.wrapping_mul(val));
    }

    fn std_dev(&self) -> f32 {
        if self.num != 0 {
            // (Unsigned 64-bit arithmetic, as in the C++.)
            ((u64::from(self.num)
                .wrapping_mul(self.total2)
                .wrapping_sub(self.total.wrapping_mul(self.total))) as f32)
                .sqrt()
                / self.num as f32
        } else {
            0.0
        }
    }
}

/// The largest standard deviation among a block's channels: how far from smooth the block is.
pub(crate) fn compute_block_max_std_dev(
    pixels: &[ColorRgba],
    block_width: u32,
    block_height: u32,
    num_comps: u32,
) -> f32 {
    let mut comp_stats = [TrackedStat::default(); 4];

    for y in 0..block_height {
        for x in 0..block_width {
            let pixel = &pixels[(x + y * block_width) as usize];
            for c in 0..num_comps as usize {
                comp_stats[c].update(u32::from(pixel[c]));
            }
        }
    }

    let mut max_std_dev = 0.0f32;
    for stat in comp_stats.iter().take(num_comps as usize) {
        // (std::max: the second when the first is less.)
        let std_dev = stat.std_dev();
        if max_std_dev < std_dev {
            max_std_dev = std_dev;
        }
    }
    max_std_dev
}

/// The mean squared error between two blocks of pixels, over the channels in use.
#[allow(clippy::too_many_arguments)]
fn compute_block_mse(
    pixels_a: &[ColorRgba],
    pixels_b: &[ColorRgba],
    block_width: u32,
    block_height: u32,
    total_block_pixels: u32,
    num_comps: u32,
    weights: &[u32; 4],
    one_over_total_color_weight: f32,
) -> f32 {
    let mut total_err = 0u64;

    // (The C++ multiplies an unsigned weight by a signed difference, so the products are worked
    // out in unsigned 32 bits, wrapping; a block's worth of them is added up in 64.)
    let d = |a: &ColorRgba, b: &ColorRgba, c: usize| (i32::from(a[c]) - i32::from(b[c])) as u32;
    let wdd = |w: u32, d: u32| w.wrapping_mul(d).wrapping_mul(d);

    if block_width == 4 && block_height == 4 && num_comps == 4 {
        if weights[0] == 1 && weights[1] == 1 && weights[2] == 1 && weights[3] == 1 {
            for i in 0..16 {
                let (a, b) = (&pixels_a[i], &pixels_b[i]);
                let dr = i32::from(a[0]) - i32::from(b[0]);
                let dg = i32::from(a[1]) - i32::from(b[1]);
                let db = i32::from(a[2]) - i32::from(b[2]);
                let da = i32::from(a[3]) - i32::from(b[3]);
                total_err += (dr * dr + dg * dg + db * db + da * da) as u64;
            }
        } else {
            for i in 0..16 {
                let (a, b) = (&pixels_a[i], &pixels_b[i]);
                total_err += u64::from(
                    wdd(weights[0], d(a, b, 0))
                        .wrapping_add(wdd(weights[1], d(a, b, 1)))
                        .wrapping_add(wdd(weights[2], d(a, b, 2)))
                        .wrapping_add(wdd(weights[3], d(a, b, 3))),
                );
            }
        }
    } else if block_width == 4 && block_height == 4 && num_comps == 3 {
        for i in 0..16 {
            let (a, b) = (&pixels_a[i], &pixels_b[i]);
            total_err += u64::from(
                wdd(weights[0], d(a, b, 0))
                    .wrapping_add(wdd(weights[1], d(a, b, 1)))
                    .wrapping_add(wdd(weights[2], d(a, b, 2))),
            );
        }
    } else if block_width == 4 && block_height == 4 && num_comps == 2 {
        for i in 0..16 {
            let (a, b) = (&pixels_a[i], &pixels_b[i]);
            total_err +=
                u64::from(wdd(weights[0], d(a, b, 0)).wrapping_add(wdd(weights[1], d(a, b, 1))));
        }
    } else if block_width == 4 && block_height == 4 && num_comps == 1 {
        for i in 0..16 {
            let (a, b) = (&pixels_a[i], &pixels_b[i]);
            total_err += u64::from(wdd(weights[0], d(a, b, 0)));
        }
    } else {
        for y in 0..block_height {
            let y_ofs = y * block_width;
            for x in 0..block_width {
                let (a, b) = (
                    &pixels_a[(x + y_ofs) as usize],
                    &pixels_b[(x + y_ofs) as usize],
                );
                for (c, &weight) in weights[..num_comps as usize].iter().enumerate() {
                    total_err += u64::from(wdd(weight, d(a, b, c)));
                }
            }
        }
    }

    total_err as f32 * (one_over_total_color_weight / total_block_pixels as f32)
}

/// Paul Hsieh's hash, as the C++ has it, of a run of bytes.
fn hash_hsieh(buf: &[u8], salt: u32) -> u32 {
    let len = buf.len();
    if len == 0 {
        return 0;
    }

    let mut h = (len as u32).wrapping_add(salt << 16);

    let bytes_left = len & 3;
    let mut at = 0usize;
    for _ in 0..len >> 2 {
        let word0 = u32::from(u16::from_le_bytes([buf[at], buf[at + 1]]));
        let word1 = u32::from(u16::from_le_bytes([buf[at + 2], buf[at + 3]]));

        h = h.wrapping_add(word0);

        let t = (word1 << 11) ^ h;
        h = (h << 16) ^ t;

        at += 4;

        h = h.wrapping_add(h >> 11);
    }

    match bytes_left {
        1 => {
            h = h.wrapping_add(buf[at] as i8 as u32);
            h ^= h << 10;
            h = h.wrapping_add(h >> 1);
        }
        2 => {
            h = h.wrapping_add(u32::from(u16::from_le_bytes([buf[at], buf[at + 1]])));
            h ^= h << 11;
            h = h.wrapping_add(h >> 17);
        }
        3 => {
            h = h.wrapping_add(u32::from(u16::from_le_bytes([buf[at], buf[at + 1]])));
            h ^= h << 16;
            h ^= (i32::from(buf[at + 2] as i8) << 18) as u32;
            h = h.wrapping_add(h >> 11);
        }
        _ => {}
    }

    h ^= h << 3;
    h = h.wrapping_add(h >> 5);
    h ^= h << 4;
    h = h.wrapping_add(h >> 17);
    h ^= h << 25;
    h = h.wrapping_add(h >> 6);

    h
}

const HASH_SIZE: usize = 8192;

/// The entropy reduction transform, with its Deflate, LZMA and LZHAM leanings.
///
/// - `blocks`: the packed blocks, `num_blocks` of them, one every
///   `total_block_stride_in_bytes` bytes. They are changed in place.
/// - `block_size_to_optimize_in_bytes`: how many bytes at the start of each stride are gone
///   over, 3 to 256. It is the stride for most formats; a BC3 block is done in two passes of 8
///   bytes at a stride of 16, the second with `blocks` starting 8 bytes in.
/// - `block_width`, `block_height`: the pixels of a block, at most 144 together.
/// - `num_comps`: the channels that count, 1 to 4.
/// - `block_pixels`: the pixels the blocks were packed from, a block after another, each in
///   rows from the top.
/// - `total_modified`: has the number of blocks changed added to it.
/// - `unpack_block`: unpacks a block (its bytes to optimize, the pixels to fill, the block's
///   index) and says whether the block is valid. A trial that makes an invalid block is
///   dropped.
/// - `block_mse_scales`: for each block, a scale for its error to use instead of the one worked
///   out from how smooth it is, where it is above 0.
///
/// False when a setting is out of range or a block being gone over does not unpack.
// (The original's hash table of recent blocks, on the stack as it has it.)
// (Its parameters are the original's, each a separate part of the state; one pass, step for step as
// the original's, its state shared throughout.)
// (The mask, as the original has it: is_multiple_of(256) runs slower in this loop.)
#[allow(clippy::verbose_bit_mask)]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
#[allow(clippy::large_stack_arrays)]
pub fn reduce_entropy<F>(
    blocks: &mut [u8],
    num_blocks: u32,
    total_block_stride_in_bytes: u32,
    block_size_to_optimize_in_bytes: u32,
    block_width: u32,
    block_height: u32,
    num_comps: u32,
    block_pixels: &[ColorRgba],
    params: &ReduceEntropyParams,
    total_modified: &mut u32,
    mut unpack_block: F,
    block_mse_scales: Option<&[f32]>,
) -> bool
where
    F: FnMut(&[u8], &mut [ColorRgba], u32) -> bool,
{
    // (Where the C++ only asserts, this says no as well.)
    if total_block_stride_in_bytes == 0
        || block_size_to_optimize_in_bytes == 0
        || total_block_stride_in_bytes < block_size_to_optimize_in_bytes
    {
        return false;
    }
    if !(1..=4).contains(&num_comps) {
        return false;
    }
    if params.color_weights[num_comps as usize..]
        .iter()
        .any(|&weight| weight != 0)
    {
        return false;
    }

    let total_color_weight = params.color_weights[0]
        .wrapping_add(params.color_weights[1])
        .wrapping_add(params.color_weights[2])
        .wrapping_add(params.color_weights[3]);
    if total_color_weight == 0 {
        return false;
    }
    let one_over_total_color_weight = 1.0f32 / total_color_weight as f32;

    if block_size_to_optimize_in_bytes < MIN_MATCH_LEN
        || block_size_to_optimize_in_bytes as usize > MAX_BLOCK_SIZE_IN_BYTES
    {
        return false;
    }

    let total_block_pixels = block_width * block_height;
    if total_block_pixels as usize > MAX_BLOCK_PIXELS {
        return false;
    }

    let stride = total_block_stride_in_bytes as usize;
    let size = block_size_to_optimize_in_bytes as usize;
    let block_pixel_count = total_block_pixels as usize;

    let total_blocks_to_check =
        (params.lookback_window_size / total_block_stride_in_bytes).max(1) as i32;

    let mut len_hist = vec![0u32; MAX_BLOCK_SIZE_IN_BYTES + 1];
    let mut second_len_hist = vec![0u32; MAX_BLOCK_SIZE_IN_BYTES + 1];
    let mut total_second_matches = 0u32;

    let mut prev_match_window_ofs_to_favor_cont = -1i32;
    // (ERT_FAVOR_REP0_MATCHES is 0 in the C++, so this never changes.)
    let prev_match_dist_to_favor = -1i32;

    let mut total_smooth_blocks = 0u32;

    let mut hash = [0u32; HASH_SIZE];

    let mut decoded_block = [[0u8; 4]; MAX_BLOCK_PIXELS];
    let mut decoded_trial_block = [[0u8; 4]; MAX_BLOCK_PIXELS];

    for block_index in 0..num_blocks {
        if block_index & 0xFF == 0 {
            hash.fill(0);
        }

        let orig_at = block_index as usize * stride;
        let pixels = &block_pixels[block_index as usize * block_pixel_count
            ..(block_index as usize + 1) * block_pixel_count];

        if !unpack_block(
            &blocks[orig_at..orig_at + size],
            &mut decoded_block[..block_pixel_count],
            block_index,
        ) {
            return false;
        }

        let cur_mse = compute_block_mse(
            pixels,
            &decoded_block,
            block_width,
            block_height,
            total_block_pixels,
            num_comps,
            &params.color_weights,
            one_over_total_color_weight,
        );

        if params.skip_zero_mse_blocks && cur_mse == 0.0 {
            continue;
        }

        let max_std_dev = compute_block_max_std_dev(pixels, block_width, block_height, num_comps);

        let mut yl = clampf(max_std_dev / params.max_smooth_block_std_dev, 0.0, 1.0);
        yl *= yl;
        let mut smooth_block_mse_scale = lerp(params.smooth_block_max_mse_scale, 1.0, yl);

        if let Some(scales) = block_mse_scales
            && scales[block_index as usize] > 0.0
        {
            smooth_block_mse_scale = scales[block_index as usize];
        }

        if smooth_block_mse_scale > 1.0 {
            total_smooth_blocks += 1;
        }

        let cur_bits = LITERAL_BITS * block_size_to_optimize_in_bytes as f32;
        let cur_t = cur_mse * smooth_block_mse_scale + cur_bits * params.lambda;

        let first_block_to_check =
            (block_index.wrapping_sub(total_blocks_to_check as u32) as i32).max(0);
        let last_block_to_check = block_index as i32 - 1;

        let mut best_block = [0u8; MAX_BLOCK_SIZE_IN_BYTES];
        best_block[..size].copy_from_slice(&blocks[orig_at..orig_at + size]);
        let mut orig_block = [0u8; MAX_BLOCK_SIZE_IN_BYTES];
        orig_block[..size].copy_from_slice(&blocks[orig_at..orig_at + size]);

        let mut best_t = cur_t;
        let (
            mut best_match_len,
            mut best_match_src_window_ofs,
            mut best_match_dst_window_ofs,
            mut best_match_dst_block_ofs,
        ) = (0u32, 0u32, 0u32, 0u32);
        let mut best_match_bits = 0.0f32;

        // Don't let thresh_ms_err be 0 to let zero error blocks have slightly increased distortion
        let thresh_ms_err = params.max_allowed_rms_increase_ratio
            * params.max_allowed_rms_increase_ratio
            * if cur_mse < 1.0 { 1.0 } else { cur_mse };

        let mut prev_block_index = last_block_to_check;
        while prev_block_index >= first_block_to_check {
            let prev_at = prev_block_index as usize * stride;

            let mut len = block_size_to_optimize_in_bytes;
            while len >= MIN_MATCH_LEN {
                if params.allow_relative_movement {
                    for src_ofs in 0..=(block_size_to_optimize_in_bytes - len) {
                        let src_match_window_ofs = (prev_block_index as u32)
                            .wrapping_mul(total_block_stride_in_bytes)
                            .wrapping_add(src_ofs);

                        for dst_ofs in 0..=(block_size_to_optimize_in_bytes - len) {
                            let dst_match_window_ofs = block_index
                                .wrapping_mul(total_block_stride_in_bytes)
                                .wrapping_add(dst_ofs);

                            let match_dist =
                                dst_match_window_ofs.wrapping_sub(src_match_window_ofs);

                            let trial_match_bits;
                            let trial_total_bits;

                            let hs = hash_hsieh(
                                &blocks[prev_at + src_ofs as usize
                                    ..prev_at + (src_ofs + len) as usize],
                                dst_ofs,
                            );

                            // Continue a previous match (which would cross block boundaries)
                            if src_match_window_ofs as i32 == prev_match_window_ofs_to_favor_cont
                                && dst_ofs == 0
                            {
                                trial_match_bits = MATCH_CONTINUE_BITS;
                                trial_total_bits = (block_size_to_optimize_in_bytes - len) as f32
                                    * LITERAL_BITS
                                    + MATCH_CONTINUE_BITS;
                            }
                            // Exploit REP0 matches
                            else if prev_match_dist_to_favor != -1
                                && src_match_window_ofs
                                    == dst_match_window_ofs
                                        .wrapping_sub(prev_match_dist_to_favor as u32)
                            {
                                trial_match_bits = MATCH_REP0_BITS;
                                trial_total_bits = (block_size_to_optimize_in_bytes - len) as f32
                                    * LITERAL_BITS
                                    + MATCH_REP0_BITS;
                            } else {
                                trial_match_bits =
                                    compute_match_cost_estimate(match_dist, len) as f32;
                                trial_total_bits = (block_size_to_optimize_in_bytes - len) as f32
                                    * LITERAL_BITS
                                    + trial_match_bits;

                                let hash_check = hash[hs as usize & (HASH_SIZE - 1)];
                                if hash_check & 0xFF == block_index & 0xFF
                                    && hash_check >> 8 == hs >> 8
                                {
                                    continue;
                                }
                            }

                            hash[hs as usize & (HASH_SIZE - 1)] =
                                (hs & 0xFFFF_FF00) | (block_index & 0xFF);

                            let trial_total_bits_times_lambda = trial_total_bits * params.lambda;

                            let mut trial_block = orig_block;
                            trial_block[dst_ofs as usize..(dst_ofs + len) as usize]
                                .copy_from_slice(
                                    &blocks[prev_at + src_ofs as usize
                                        ..prev_at + (src_ofs + len) as usize],
                                );

                            if !unpack_block(
                                &trial_block[..size],
                                &mut decoded_trial_block[..block_pixel_count],
                                block_index,
                            ) {
                                continue;
                            }

                            let trial_mse = compute_block_mse(
                                pixels,
                                &decoded_trial_block,
                                block_width,
                                block_height,
                                total_block_pixels,
                                num_comps,
                                &params.color_weights,
                                one_over_total_color_weight,
                            );

                            if trial_mse < thresh_ms_err {
                                let t = trial_mse * smooth_block_mse_scale
                                    + trial_total_bits_times_lambda;

                                if t < best_t {
                                    best_t = t;
                                    best_block = trial_block;
                                    best_match_len = len;
                                    best_match_src_window_ofs = src_match_window_ofs;
                                    best_match_dst_window_ofs = dst_match_window_ofs;
                                    best_match_dst_block_ofs = dst_ofs;
                                    best_match_bits = trial_match_bits;
                                }
                            }
                        } // dst_ofs
                    } // src_ofs
                } else {
                    let match_dist = block_index
                        .wrapping_sub(prev_block_index as u32)
                        .wrapping_mul(total_block_stride_in_bytes);

                    // Assume the block has 1 match and
                    // block_size_to_optimize_in_bytes-match_len literals.
                    let trial_match_bits = compute_match_cost_estimate(match_dist, len) as f32;
                    let trial_total_bits = (block_size_to_optimize_in_bytes - len) as f32
                        * LITERAL_BITS
                        + trial_match_bits;
                    let trial_total_bits_times_lambda = trial_total_bits * params.lambda;

                    for ofs in 0..=(block_size_to_optimize_in_bytes - len) {
                        let dst_match_window_ofs = block_index
                            .wrapping_mul(total_block_stride_in_bytes)
                            .wrapping_add(ofs);
                        let src_match_window_ofs = (prev_block_index as u32)
                            .wrapping_mul(total_block_stride_in_bytes)
                            .wrapping_add(ofs);

                        let mut trial_match_bits_to_use = trial_match_bits;
                        let mut trial_total_bits_times_lambda_to_use =
                            trial_total_bits_times_lambda;

                        let hs = hash_hsieh(
                            &blocks[prev_at + ofs as usize..prev_at + (ofs + len) as usize],
                            ofs,
                        );

                        // Continue a previous match (which would cross block boundaries)
                        if src_match_window_ofs as i32 == prev_match_window_ofs_to_favor_cont
                            && ofs == 0
                        {
                            let continue_match_trial_bits =
                                (block_size_to_optimize_in_bytes - len) as f32 * LITERAL_BITS
                                    + MATCH_CONTINUE_BITS;
                            trial_match_bits_to_use = MATCH_CONTINUE_BITS;
                            trial_total_bits_times_lambda_to_use =
                                continue_match_trial_bits * params.lambda;
                        }
                        // Exploit REP0 matches
                        else if prev_match_dist_to_favor != -1
                            && src_match_window_ofs
                                == dst_match_window_ofs
                                    .wrapping_sub(prev_match_dist_to_favor as u32)
                        {
                            let continue_match_trial_bits =
                                (block_size_to_optimize_in_bytes - len) as f32 * LITERAL_BITS
                                    + MATCH_REP0_BITS;
                            trial_match_bits_to_use = MATCH_REP0_BITS;
                            trial_total_bits_times_lambda_to_use =
                                continue_match_trial_bits * params.lambda;
                        } else {
                            let hash_check = hash[hs as usize & (HASH_SIZE - 1)];
                            if hash_check & 0xFF == block_index & 0xFF && hash_check >> 8 == hs >> 8
                            {
                                continue;
                            }
                        }

                        hash[hs as usize & (HASH_SIZE - 1)] =
                            (hs & 0xFFFF_FF00) | (block_index & 0xFF);

                        let mut trial_block = orig_block;
                        trial_block[ofs as usize..(ofs + len) as usize].copy_from_slice(
                            &blocks[prev_at + ofs as usize..prev_at + (ofs + len) as usize],
                        );

                        if !unpack_block(
                            &trial_block[..size],
                            &mut decoded_trial_block[..block_pixel_count],
                            block_index,
                        ) {
                            continue;
                        }

                        let trial_mse = compute_block_mse(
                            pixels,
                            &decoded_trial_block,
                            block_width,
                            block_height,
                            total_block_pixels,
                            num_comps,
                            &params.color_weights,
                            one_over_total_color_weight,
                        );

                        if trial_mse < thresh_ms_err {
                            let t = trial_mse * smooth_block_mse_scale
                                + trial_total_bits_times_lambda_to_use;

                            if t < best_t {
                                best_t = t;
                                best_block = trial_block;
                                best_match_len = len;
                                best_match_src_window_ofs = src_match_window_ofs;
                                best_match_dst_window_ofs = dst_match_window_ofs;
                                best_match_dst_block_ofs = ofs;
                                best_match_bits = trial_match_bits_to_use;
                            }
                        }
                    } // ofs
                }

                len -= 1;
            } // len

            prev_block_index -= 1;
        } // prev_block_index

        if best_t < cur_t {
            let (
                mut best_second_match_len,
                mut best_second_match_src_window_ofs,
                mut best_second_match_dst_window_ofs,
                mut best_second_match_dst_block_ofs,
            ) = (0u32, 0u32, 0u32, 0u32);

            // Try injecting a second match, being sure it does't overlap with the first.
            if params.try_two_matches && best_match_len <= block_size_to_optimize_in_bytes - 3 {
                let mut matched_flags = [0u8; MAX_BLOCK_SIZE_IN_BYTES];
                matched_flags[best_match_dst_block_ofs as usize
                    ..(best_match_dst_block_ofs + best_match_len) as usize]
                    .fill(1);

                let orig_best_block = best_block;

                let mut prev_block_index = last_block_to_check;
                while prev_block_index >= first_block_to_check {
                    let prev_at = prev_block_index as usize * stride;

                    let match_dist = block_index
                        .wrapping_sub(prev_block_index as u32)
                        .wrapping_mul(total_block_stride_in_bytes);

                    for len in 3..=(block_size_to_optimize_in_bytes - best_match_len) {
                        let trial_total_bits =
                            (block_size_to_optimize_in_bytes - len - best_match_len) as f32
                                * LITERAL_BITS
                                + compute_match_cost_estimate(match_dist, len) as f32
                                + best_match_bits;

                        let trial_total_bits_times_lambda = trial_total_bits * params.lambda;

                        for ofs in 0..=(block_size_to_optimize_in_bytes - len) {
                            if matched_flags[ofs as usize..(ofs + len) as usize]
                                .iter()
                                .any(|&flag| flag != 0)
                            {
                                continue;
                            }

                            let dst_match_window_ofs = block_index
                                .wrapping_mul(total_block_stride_in_bytes)
                                .wrapping_add(ofs);
                            let src_match_window_ofs = (prev_block_index as u32)
                                .wrapping_mul(total_block_stride_in_bytes)
                                .wrapping_add(ofs);

                            let mut trial_block = orig_best_block;
                            trial_block[ofs as usize..(ofs + len) as usize].copy_from_slice(
                                &blocks[prev_at + ofs as usize..prev_at + (ofs + len) as usize],
                            );

                            if !unpack_block(
                                &trial_block[..size],
                                &mut decoded_trial_block[..block_pixel_count],
                                block_index,
                            ) {
                                continue;
                            }

                            let trial_mse = compute_block_mse(
                                pixels,
                                &decoded_trial_block,
                                block_width,
                                block_height,
                                total_block_pixels,
                                num_comps,
                                &params.color_weights,
                                one_over_total_color_weight,
                            );

                            if trial_mse < thresh_ms_err {
                                let t = trial_mse * smooth_block_mse_scale
                                    + trial_total_bits_times_lambda;

                                if t < best_t {
                                    best_t = t;
                                    best_block = trial_block;
                                    best_second_match_len = len;
                                    best_second_match_src_window_ofs = src_match_window_ofs;
                                    best_second_match_dst_window_ofs = dst_match_window_ofs;
                                    best_second_match_dst_block_ofs = ofs;
                                }
                            }
                        }
                    }

                    prev_block_index -= 1;
                }
            }

            blocks[orig_at..orig_at + size].copy_from_slice(&best_block[..size]);
            *total_modified += 1;

            if best_second_match_len == 0
                || best_match_dst_window_ofs > best_second_match_dst_window_ofs
            {
                if block_size_to_optimize_in_bytes == total_block_stride_in_bytes {
                    // If the match goes all the way to the end of a block,
                    // we can try to continue it on the next encoded block.
                    if best_match_dst_block_ofs + best_match_len == total_block_stride_in_bytes {
                        prev_match_window_ofs_to_favor_cont =
                            best_match_src_window_ofs.wrapping_add(best_match_len) as i32;
                    } else {
                        prev_match_window_ofs_to_favor_cont = -1;
                    }
                }
            } else if block_size_to_optimize_in_bytes == total_block_stride_in_bytes {
                // If the match goes all the way to the end of a block,
                // we can try to continue it on the next encoded block.
                if best_second_match_dst_block_ofs + best_second_match_len
                    == total_block_stride_in_bytes
                {
                    prev_match_window_ofs_to_favor_cont =
                        best_second_match_src_window_ofs.wrapping_add(best_second_match_len) as i32;
                } else {
                    prev_match_window_ofs_to_favor_cont = -1;
                }
            }

            len_hist[best_match_len as usize] += 1;

            if best_second_match_len != 0 {
                second_len_hist[best_second_match_len as usize] += 1;
                total_second_matches += 1;
            }
        } else {
            prev_match_window_ofs_to_favor_cont = -1;
        }
    } // block_index

    if params.debug_output {
        println!(
            "Total smooth blocks: {:3.2}%",
            total_smooth_blocks as f32 * 100.0 / num_blocks as f32
        );

        println!("Match length histogram:");
        for i in MIN_MATCH_LEN..=block_size_to_optimize_in_bytes {
            print!(
                "{}{}",
                len_hist[i as usize],
                if i < block_size_to_optimize_in_bytes {
                    ','
                } else {
                    '\n'
                }
            );
        }

        println!(
            "Total second matches: {} {:3.2}%",
            total_second_matches,
            total_second_matches as f32 * 100.0 / num_blocks as f32
        );
        println!("Secod match length histogram:");
        for i in MIN_MATCH_LEN..=block_size_to_optimize_in_bytes {
            print!(
                "{}{}",
                second_len_hist[i as usize],
                if i < block_size_to_optimize_in_bytes {
                    ','
                } else {
                    '\n'
                }
            );
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bc7decomp::unpack_bc7;
    use crate::bc7enc::{Bc7Enc, CompressBlockParams};

    fn unpack(block: &[u8], pixels: &mut [ColorRgba], _: u32) -> bool {
        unpack_bc7(block.try_into().unwrap(), pixels.try_into().unwrap())
    }

    /// Two blocks packed as BC7: a ramp, and the same ramp with one pixel a step off.
    fn two_near_blocks() -> (Vec<ColorRgba>, Vec<u8>) {
        let mut pixels = Vec::new();
        for block in 0..2u8 {
            for i in 0..16u8 {
                pixels.push([10 + i * 12, 200 - i * 9, 40 + i * 3, 255]);
            }
            if block == 1 {
                pixels[16 + 5][1] += 1;
            }
        }
        let encoder = Bc7Enc::new();
        let params = CompressBlockParams::new();
        let mut packed = vec![0u8; 32];
        for (block, pixels) in packed
            .as_chunks_mut::<16>()
            .0
            .iter_mut()
            .zip(pixels.as_chunks::<16>().0.iter())
        {
            encoder.compress_block(block, pixels, &params).unwrap();
        }
        (pixels, packed)
    }

    #[test]
    fn a_near_copy_becomes_a_copy_when_bits_weigh_enough() {
        let (pixels, mut packed) = two_near_blocks();
        let mut params = ReduceEntropyParams::new();
        params.lambda = 50.0;
        let mut modified = 0;
        assert!(reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            4,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        assert_eq!(modified, 1);
        let (first, second) = packed.split_at(16);
        assert_eq!(first, second);
    }

    #[test]
    fn nothing_changes_when_bits_weigh_nothing() {
        let (pixels, mut packed) = two_near_blocks();
        let before = packed.clone();
        let mut params = ReduceEntropyParams::new();
        params.lambda = 0.0;
        let mut modified = 0;
        assert!(reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            4,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        // (With no weight on the bits a trial has to lower the error to be kept, and a block
        // cannot be nearer its own pixels than the encoder left it by taking another's bytes
        // here.)
        assert_eq!(modified, 0);
        assert_eq!(packed, before);
    }

    #[test]
    fn the_first_block_is_never_changed() {
        let (pixels, mut packed) = two_near_blocks();
        let first = packed[..16].to_vec();
        let mut params = ReduceEntropyParams::new();
        params.lambda = 1000.0;
        params.try_two_matches = true;
        params.allow_relative_movement = true;
        let mut modified = 0;
        assert!(reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            4,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        assert_eq!(&packed[..16], &first[..]);
        // What is left still unpacks.
        let mut unpacked = [[0u8; 4]; 16];
        assert!(unpack_bc7(packed[16..].try_into().unwrap(), &mut unpacked));
    }

    #[test]
    fn settings_out_of_range_are_refused() {
        let (pixels, mut packed) = two_near_blocks();
        let params = ReduceEntropyParams::new();
        let mut modified = 0;
        // A weight on a channel that is not in use.
        assert!(!reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            3,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        // Fewer than three bytes to go over, and more bytes to go over than the stride.
        assert!(!reduce_entropy(
            &mut packed,
            2,
            16,
            2,
            4,
            4,
            4,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        assert!(!reduce_entropy(
            &mut packed,
            2,
            8,
            16,
            4,
            4,
            4,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        // No channels, and no weight at all.
        assert!(!reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            0,
            &pixels,
            &params,
            &mut modified,
            unpack,
            None
        ));
        let mut weightless = params;
        weightless.color_weights = [0; 4];
        assert!(!reduce_entropy(
            &mut packed,
            2,
            16,
            16,
            4,
            4,
            4,
            &pixels,
            &weightless,
            &mut modified,
            unpack,
            None
        ));
        assert_eq!(modified, 0);
    }

    #[test]
    fn the_settings_print_as_the_cpp_prints_them() {
        let printed = ReduceEntropyParams::new().to_string();
        assert!(
            printed.starts_with("lambda: 1.000000\nLookback window size: 256\n"),
            "{printed}"
        );
        assert!(printed.ends_with("Skip zero MSE blocks: 0\n"), "{printed}");
    }
}
