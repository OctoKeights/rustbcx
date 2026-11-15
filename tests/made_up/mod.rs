//! Made-up blocks, the same from a seed here as in `compare/dump.cpp`, and the dumps the tests
//! run as well as the comparison (`examples/dump.rs`).

// (The arithmetic is written as dump.cpp has it; not every user of this module uses all of it.)
#![allow(clippy::manual_is_multiple_of, clippy::needless_range_loop, dead_code)]

use rustbcx::bc2;
use rustbcx::bc6h::{self, Bc6hEncSettings};
use rustbcx::bc7decomp;
use rustbcx::bc7enc::{Bc7Enc, CompressBlockParams};
use rustbcx::dds;
use rustbcx::ert::{self, ReduceEntropyParams};
use rustbcx::image::Image;
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcEncoder, RdoBcParams};
use rustbcx::rgbcx::{self, Bc1ApproxMode, Rgbcx};
use rustbcx::texture::Texture;

/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }
}

/// FNV-1a, 64 bits.
pub struct Hash {
    pub value: u64,
}

impl Hash {
    pub fn new() -> Hash {
        Hash { value: 14695981039346656037 }
    }

    pub fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.value ^= u64::from(byte);
            self.value = self.value.wrapping_mul(1099511628211);
        }
    }
}

/// Sixteen made-up pixels of one of eight kinds, the same in both programs.
pub fn make_pixels(random: &mut Random) -> [[u8; 4]; 16] {
    let mut pixels = [[0u8; 4]; 16];
    let kind = random.next() % 8;
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    match kind {
        // noise
        0 => {
            for pixel in &mut pixels {
                for value in pixel {
                    *value = random.next() as u8;
                }
            }
        }
        // from one colour to another, with a little noise
        1 => {
            let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
            for c in 0..4 {
                a[c] = random.next() as u8;
                b[c] = random.next() as u8;
            }
            let noise = random.next() % 9;
            for pixel in &mut pixels {
                let t = (random.next() % 256) as i32;
                for c in 0..4 {
                    let n = if noise != 0 { (random.next() % (noise + 1)) as i32 - (noise / 2) as i32 } else { 0 };
                    pixel[c] = clamp((i32::from(a[c]) * (255 - t) + i32::from(b[c]) * t) / 255 + n);
                }
            }
        }
        // greys
        2 => {
            for pixel in &mut pixels {
                let v = random.next() as u8;
                *pixel = [v, v, v, random.next() as u8];
            }
        }
        // greys one step apart
        3 => {
            let base = random.next() % 255;
            for pixel in &mut pixels {
                let v = (base + random.next() % 2) as u8;
                *pixel = [v, v, v, (base + random.next() % 3) as u8];
            }
        }
        // one colour, or one colour but for a pixel
        4 | 5 => {
            let mut a = [0u8; 4];
            for value in &mut a {
                *value = random.next() as u8;
            }
            pixels = [a; 16];
            if kind == 5 {
                let at = (random.next() % 16) as usize;
                for c in 0..4 {
                    pixels[at][c] = (u32::from(a[c]) + 1 + random.next() % 40) as u8;
                }
            }
        }
        // dark, with pixels that count as black
        6 => {
            for pixel in &mut pixels {
                let bright = random.next() % 4 == 0;
                for value in pixel {
                    *value = if bright { random.next() as u8 } else { (random.next() % 8) as u8 };
                }
            }
        }
        // three colours
        _ => {
            let mut palette = [[0u8; 4]; 3];
            for color in &mut palette {
                for value in color {
                    *value = random.next() as u8;
                }
            }
            for pixel in &mut pixels {
                *pixel = palette[(random.next() % 3) as usize];
            }
        }
    }
    pixels
}

/// The `ert` dump: runs of made-up blocks, many of them near copies of the blocks before,
/// packed in one of five formats and put through the entropy reduction transform with its
/// settings varied. What `compare/dump.cpp` prints for the same seed and count.
pub fn ert(seed: u64, blocks: u32) -> String {
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let rgbcx_encoder = Rgbcx::new(Bc1ApproxMode::Ideal);
    let bc7_encoder = Bc7Enc::new();
    let bc7_params = CompressBlockParams::new();
    let (mut remaining, mut runs, mut modified, mut failed) = (blocks, 0u32, 0u32, 0u32);
    while remaining > 0 {
        let format = random.next() % 5;
        let relative = random.next() % 8 == 0;
        let mut n = if relative { 8 + random.next() % 57 } else { 32 + random.next() % 481 };
        if n > remaining {
            n = remaining;
        }
        remaining -= n;
        let n = n as usize;

        let mut px: Vec<[[u8; 4]; 16]> = Vec::with_capacity(n);
        for i in 0..n {
            let r = random.next() % 8;
            let mut block = if i > 0 && r < 3 {
                px[i - 1]
            } else if i > 3 && r == 3 {
                px[i - (1 + random.next() % 3) as usize]
            } else {
                make_pixels(&mut random)
            };
            if i > 0 && r < 2 {
                let at = (random.next() % 64) as usize;
                block[at / 4][at % 4] = block[at / 4][at % 4].wrapping_add(1);
            }
            px.push(block);
        }

        let mut p = ReduceEntropyParams::new();
        p.lambda = (random.next() % 2000) as f32 / 100.0 + 0.01;
        const WINDOWS: [u32; 6] = [16, 48, 128, 256, 1024, 4096];
        p.lookback_window_size = WINDOWS[(random.next() % 6) as usize];
        p.max_allowed_rms_increase_ratio = 1.0 + (random.next() % 1000) as f32 / 100.0;
        p.max_smooth_block_std_dev = 1.0 + (random.next() % 40) as f32;
        p.smooth_block_max_mse_scale = 1.0 + (random.next() % 20) as f32;
        p.try_two_matches = random.next() % 2 != 0;
        p.allow_relative_movement = relative;
        p.skip_zero_mse_blocks = random.next() % 4 == 0;
        let mut weights = [1u32; 4];
        if random.next() % 2 != 0 {
            for weight in &mut weights {
                *weight = 1 + random.next() % 4;
            }
        }
        let use_scales = random.next() % 3 == 0;
        let mut scales = vec![0.0f32; n];
        if use_scales {
            for scale in &mut scales {
                let a = random.next();
                *scale = if a % 4 == 0 { 0.0 } else { 1.0 + ((a >> 8) % 10) as f32 };
            }
        }
        let scale_list = if use_scales { Some(&scales[..]) } else { None };
        let pixels = px.as_flattened();

        let stride = if format < 2 { 8usize } else { 16 };
        let mut packed = vec![0u8; n * stride];
        let level = random.next() % 6;
        let mut total = 0u32;
        let set_weights = |p: &mut ReduceEntropyParams, comps: usize| {
            for c in 0..4 {
                p.color_weights[c] = if c < comps { weights[c] } else { 0 };
            }
        };
        let unpack_bc1 = |block: &[u8], pixels: &mut [[u8; 4]], _: u32| {
            rgbcx::unpack_bc1(block.try_into().unwrap(), pixels.try_into().unwrap(), true, Bc1ApproxMode::Ideal);
            true
        };
        let unpack_bc4 = |block: &[u8], pixels: &mut [[u8; 4]], _: u32| {
            rgbcx::unpack_bc4(block.try_into().unwrap(), pixels.as_flattened_mut(), 4);
            true
        };
        let ok = match format {
            0 => {
                for (block, pixels) in packed.chunks_exact_mut(8).zip(&px) {
                    rgbcx_encoder.encode_bc1(level, block.try_into().unwrap(), pixels, false, false, None);
                }
                set_weights(&mut p, 3);
                ert::reduce_entropy(&mut packed, n as u32, 8, 8, 4, 4, 3, pixels, &p, &mut total, unpack_bc1, scale_list)
            }
            1 => {
                for (block, pixels) in packed.chunks_exact_mut(8).zip(&px) {
                    rgbcx::encode_bc4(block.try_into().unwrap(), pixels.as_flattened(), 4);
                }
                set_weights(&mut p, 1);
                ert::reduce_entropy(&mut packed, n as u32, 8, 8, 4, 4, 1, pixels, &p, &mut total, unpack_bc4, scale_list)
            }
            2 => {
                for (block, pixels) in packed.chunks_exact_mut(16).zip(&px) {
                    bc7_encoder.compress_block(block.try_into().unwrap(), pixels, &bc7_params);
                }
                set_weights(&mut p, 4);
                ert::reduce_entropy(&mut packed, n as u32, 16, 16, 4, 4, 4, pixels, &p, &mut total, |block, pixels, _| bc7decomp::unpack_bc7(block.try_into().unwrap(), pixels.try_into().unwrap()), scale_list)
            }
            3 => {
                for (block, pixels) in packed.chunks_exact_mut(16).zip(&px) {
                    rgbcx_encoder.encode_bc3(level, block.try_into().unwrap(), pixels);
                }
                // The alpha half, against the alpha of the pixels put in the first channel.
                let alpha: Vec<[u8; 4]> = pixels.iter().map(|pixel| [pixel[3], 0, 0, 0]).collect();
                set_weights(&mut p, 1);
                let alpha_ok = ert::reduce_entropy(&mut packed, n as u32, 16, 8, 4, 4, 1, &alpha, &p, &mut total, unpack_bc4, scale_list);
                // The colour half.
                set_weights(&mut p, 3);
                ert::reduce_entropy(&mut packed[8..], n as u32, 16, 8, 4, 4, 3, pixels, &p, &mut total, unpack_bc1, scale_list) && alpha_ok
            }
            _ => {
                for (block, pixels) in packed.chunks_exact_mut(16).zip(&px) {
                    rgbcx::encode_bc5(block.try_into().unwrap(), pixels.as_flattened(), 0, 1, 4);
                }
                set_weights(&mut p, 2);
                ert::reduce_entropy(
                    &mut packed,
                    n as u32,
                    16,
                    16,
                    4,
                    4,
                    2,
                    pixels,
                    &p,
                    &mut total,
                    |block, pixels, _| {
                        rgbcx::unpack_bc5(block.try_into().unwrap(), pixels.as_flattened_mut(), 0, 1, 4);
                        true
                    },
                    scale_list,
                )
            }
        };
        hash.add(&packed);
        modified += total;
        failed += u32::from(!ok);
        runs += 1;
    }
    format!("ert {:016x}\nert runs: {runs}, blocks changed: {modified}, runs refused: {failed}\n", hash.value)
}

/// A made-up image: noise, a ramp, flat boxes, a repeated tile, a very smooth field, or half a
/// ramp and half noise; opaque, with noise for alpha, or with a ramp of alpha.
pub fn make_image(random: &mut Random, w: u32, h: u32) -> Image {
    let mut image = Image::new(w, h);
    let kind = random.next() % 6;
    let alpha_kind = random.next() % 3;
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
    for c in 0..4 {
        a[c] = random.next() as u8;
        b[c] = random.next() as u8;
    }
    let noise = random.next() % 4;
    let mut tile = [0u8; 256];
    for value in &mut tile {
        *value = random.next() as u8;
    }
    for y in 0..h {
        for x in 0..w {
            let mut p = [0u8; 4];
            match kind {
                0 => {
                    for c in 0..4 {
                        p[c] = random.next() as u8;
                    }
                }
                1 => {
                    let t = ((x * 255) / if w > 1 { w - 1 } else { 1 }) as i32;
                    for c in 0..4 {
                        let n = if noise != 0 { (random.next() % (noise + 1)) as i32 } else { 0 };
                        p[c] = clamp((i32::from(a[c]) * (255 - t) + i32::from(b[c]) * t) / 255 + n);
                    }
                }
                2 => {
                    let in_box = ((x / 16) + (y / 12)) % 3 == 0;
                    for c in 0..4 {
                        p[c] = if in_box { b[c] } else { a[c] };
                    }
                }
                3 => {
                    for c in 0..4 {
                        p[c] = tile[(((x % 8) + (y % 8) * 8) * 4) as usize + c];
                    }
                }
                4 => {
                    for c in 0..4 {
                        p[c] = (u32::from(a[c]) / 2 + 60 + random.next() % 2) as u8;
                    }
                }
                _ => {
                    if x < w / 2 {
                        let t = ((y * 255) / if h > 1 { h - 1 } else { 1 }) as i32;
                        for c in 0..4 {
                            p[c] = ((i32::from(a[c]) * (255 - t) + i32::from(b[c]) * t) / 255) as u8;
                        }
                    } else {
                        for c in 0..4 {
                            p[c] = random.next() as u8;
                        }
                    }
                }
            }
            if alpha_kind == 0 {
                p[3] = 255;
            } else if alpha_kind == 2 {
                p[3] = ((x + y) * 3) as u8;
            }
            image.pixels[(x + y * w) as usize] = p;
        }
    }
    image
}

/// The `rdo` dump (and `rdo_bc7e`, with `with_bc7e`): made-up images of every shape packed
/// whole by the image encoder in each of its formats, with and without the rate-distortion
/// pass, every setting varied, and the `.dds` file for each. What `compare/dump.cpp` prints for
/// the same seed and count.
pub fn rdo(seed: u64, blocks: u32, with_bc7e: bool) -> String {
    const MODES: [Bc1ApproxMode; 4] = [Bc1ApproxMode::Ideal, Bc1ApproxMode::NVidia, Bc1ApproxMode::Amd, Bc1ApproxMode::IdealRound4];
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let (mut done, mut images, mut refused) = (0u32, 0u32, 0u32);
    let mut encoder = RdoBcEncoder::new();
    while done < blocks {
        let big = random.next() % 6 == 0;
        let w = if big { 240 + random.next() % 100 } else { 1 + random.next() % 96 };
        let h = if big { 240 + random.next() % 100 } else { 1 + random.next() % 96 };
        let image = make_image(&mut random, w, h);

        let mut p = RdoBcParams::new();
        const FORMATS: [BlockFormat; 5] = [BlockFormat::Bc1, BlockFormat::Bc3, BlockFormat::Bc4, BlockFormat::Bc5, BlockFormat::Bc7];
        p.format = FORMATS[(random.next() % 5) as usize];
        let level = random.next();
        p.bc1_quality_level = if big { level % 3 } else { level % 11 };
        p.use_bc1_3color_mode = random.next() % 2 != 0;
        p.use_bc1_3color_mode_for_black = random.next() % 2 != 0;
        p.bc1_mode = MODES[(random.next() % 4) as usize];
        p.y_flip = random.next() % 4 == 0;
        p.bc45_channel0 = random.next() % 4;
        p.bc45_channel1 = random.next() % 4;
        p.use_hq_bc345 = random.next() % 2 != 0;
        p.bc345_search_rad = random.next() % 8;
        p.bc345_mode_mask = 1 + random.next() % 3;
        let use_bc7e = random.next() % 2 != 0;
        p.use_bc7e = with_bc7e && use_bc7e;
        let uber = random.next();
        p.bc7_uber_level = if big {
            uber % 2
        } else if w * h <= 256 {
            uber % 7
        } else {
            uber % 5
        };
        p.bc7enc_max_partitions_to_scan = random.next() % 65;
        p.perceptual = random.next() % 2 != 0;
        p.bc7enc_mode6_only = random.next() % 8 == 0;
        p.bc7enc_reduce_entropy = random.next() % 2 != 0;
        p.bc7enc_rdo_bc7_quant_mode6_endpoints = random.next() % 2 != 0;
        p.bc7enc_rdo_bc7_weight_modes = random.next() % 2 != 0;
        p.bc7enc_rdo_bc7_weight_low_frequency_partitions = random.next() % 2 != 0;
        p.bc7enc_rdo_bc7_pbit1_weighting = random.next() % 2 != 0;
        let lambda_on = random.next() % 3 != 0;
        let lambda = random.next();
        p.rdo_lambda = if lambda_on { (lambda % 1000) as f32 / 100.0 + 0.05 } else { 0.0 };
        const WINDOWS: [u32; 6] = [8, 16, 64, 128, 256, 1024];
        p.lookback_window_size = WINDOWS[(random.next() % 6) as usize];
        p.custom_rdo_smooth_block_error_scale = random.next() % 2 != 0;
        p.rdo_smooth_block_error_scale = 1.0 + (random.next() % 40) as f32;
        p.rdo_max_smooth_block_std_dev = 1.0 + (random.next() % 40) as f32;
        let relative = random.next() % 4 == 0;
        p.rdo_allow_relative_movement = relative && !big && w * h <= 1024;
        p.rdo_try_2_matches = random.next() % 2 != 0;
        p.rdo_ultrasmooth_block_handling = random.next() % 2 != 0;
        p.rdo_multithreading = random.next() % 4 != 0;
        p.rdo_max_threads = 1 + random.next() % 8;
        // (The number of threads does not change the blocks: all the cores, one, three.)
        p.threads = [0, 1, 3][(images % 3) as usize];

        let mut ok = encoder.init(&image, &p) && encoder.encode();
        let unpacked = if ok { encoder.unpack_blocks() } else { None };
        ok = ok && unpacked.is_some();

        for size in [encoder.blocks_x(), encoder.blocks_y(), encoder.bytes_per_block(), u32::from(ok)] {
            hash.add(&size.to_le_bytes());
        }
        if let Some(unpacked) = unpacked {
            hash.add(encoder.prerdo_blocks());
            hash.add(encoder.blocks());
            hash.add(unpacked.as_rgba8());
            // The .dds file for it, with the DX10 header forced for every other image.
            let texture = Texture::new(p.format.into(), encoder.orig_width(), encoder.orig_height(), vec![encoder.blocks().to_vec()]);
            hash.add(&dds::write(&texture, images % 2 == 1).expect("a whole texture"));
        } else {
            refused += 1;
        }
        done += encoder.total_blocks();
        images += 1;
    }
    format!("rdo {:016x}\nrdo images: {images}, blocks: {done}, refused: {refused}\n", hash.value)
}

/// The first bits of the fourteen modes of BC6H.
const BC6H_MODE_BITS: [u8; 14] = [0x00, 0x01, 0x02, 0x06, 0x0a, 0x0e, 0x12, 0x16, 0x1a, 0x1e, 0x03, 0x07, 0x0b, 0x0f];

const MODES: [Bc1ApproxMode; 4] = [Bc1ApproxMode::Ideal, Bc1ApproxMode::NVidia, Bc1ApproxMode::Amd, Bc1ApproxMode::IdealRound4];

/// The `bc2` dump: made-up BC2 blocks unpacked each way of interpolating, and made-up pixels
/// packed as BC2 at every level and with flags, and unpacked again.
pub fn bc2(seed: u64, blocks: u32) -> String {
    let encoders = MODES.map(Rgbcx::new);
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut three = 0u32;
    let mut out = String::new();
    let first_not_larger = |block: &[u8; 16]| u32::from(u16::from_le_bytes([block[8], block[9]]) <= u16::from_le_bytes([block[10], block[11]]));
    for i in 0..blocks {
        let mode = MODES[((i / 100) % 4) as usize];
        let encoder = &encoders[((i / 100) % 4) as usize];

        let mut block = [0u8; 16];
        for byte in &mut block {
            *byte = random.next() as u8;
        }
        if i % 8 == 3 {
            block[10] = block[8];
            block[11] = block[9];
        }
        let mut unpacked = [[0u8; 4]; 16];
        for each in MODES {
            bc2::unpack_bc2(&block, &mut unpacked, each);
            hash.add(unpacked.as_flattened());
        }

        let pixels = make_pixels(&mut random);
        let options = random.next();
        let level = options % 19;

        let mut block = [0xEEu8; 16];
        bc2::encode_bc2(encoder, level, &mut block, &pixels);
        hash.add(&block);
        three += first_not_larger(&block);
        bc2::unpack_bc2(&block, &mut unpacked, mode);
        hash.add(unpacked.as_flattened());

        // (Without a need for the block's error the encoder takes one first guess only.)
        let mut flags = options >> 16;
        if flags & (rgbcx::ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS | rgbcx::ENCODE_BC1_USE_FULL_MSE_EVAL) == 0 {
            flags &= !rgbcx::ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS;
        }
        let mut block = [0xEEu8; 16];
        bc2::encode_bc2_with_flags(encoder, &mut block, &pixels, flags, 1 + (options >> 4) % 12);
        hash.add(&block);
        three += first_not_larger(&block);

        if (i + 1) % 20000 == 0 || i + 1 == blocks {
            out += &format!("bc2 {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("bc2 blocks packed with the first colour not the larger: {three}\n")
}

/// The `bc6h_unpack` dump: made-up BC6H blocks of every mode (and of none) unpacked as unsigned
/// and as signed, to halves and to floats. What `compare/dump_bc6h.cpp` prints for the same
/// seed and count.
pub fn bc6h_unpack(seed: u64, blocks: u32) -> String {
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut black = 0u32;
    for i in 0..blocks {
        let mut block = [0u8; 16];
        for byte in &mut block {
            *byte = random.next() as u8;
        }
        // Half of them of a mode in turn; a few with only small values in them.
        if i % 2 == 0 {
            let bits = BC6H_MODE_BITS[((i / 2) % 14) as usize];
            block[0] = if bits < 2 { (block[0] & 0xfc) | bits } else { (block[0] & 0xe0) | bits };
        }
        if i % 16 == 5 {
            for byte in &mut block[1..10] {
                *byte &= random.next() as u8;
            }
        }
        for is_signed in [false, true] {
            let halves = bc6h::unpack_bc6h_half(&block, is_signed);
            for value in halves.as_flattened() {
                hash.add(&value.to_le_bytes());
            }
            let floats = bc6h::unpack_bc6h_float(&block, is_signed);
            for value in floats.as_flattened() {
                hash.add(&value.to_bits().to_le_bytes());
            }
            black += u32::from(halves.as_flattened().iter().all(|&value| value == 0));
        }
    }
    format!("bc6h_unpack {:016x}\nbc6h_unpack blocks unpacked to black: {black}\n", hash.value)
}

/// Sixteen made-up HDR pixels, four halves each (the fourth is not packed): noise over all the
/// halves, values near one, a ramp between two colours, one colour, tiny values, two colours,
/// a smooth patch, very bright values.
pub fn make_hdr_block(r: &mut Random) -> [bc6h::RgbHalf; 16] {
    let kind = r.next() % 8;
    let mut px = [[0u16; 4]; 16];
    let half = |r: &mut Random, top: u32| (r.next() % (top + 1)) as u16;
    match kind {
        0 => {
            for value in px.as_flattened_mut() {
                *value = half(r, 0x7bff);
            }
        }
        1 => {
            for value in px.as_flattened_mut() {
                *value = (0x3800 + r.next() % 0x800) as u16;
            }
        }
        2 => {
            let (mut a, mut b) = ([0u32; 3], [0u32; 3]);
            for c in 0..3 {
                a[c] = u32::from(half(r, 0x7bff));
                b[c] = u32::from(half(r, 0x7bff));
            }
            for pixel in &mut px {
                let t = r.next() % 256;
                for c in 0..3 {
                    pixel[c] = ((a[c] * (255 - t) + b[c] * t) / 255) as u16;
                }
                pixel[3] = 0x3c00;
            }
        }
        3 => {
            let mut a = [0u16; 3];
            for value in &mut a {
                *value = half(r, 0x7bff);
            }
            for pixel in &mut px {
                *pixel = [a[0], a[1], a[2], 0x3c00];
            }
        }
        4 => {
            for value in px.as_flattened_mut() {
                *value = half(r, 40);
            }
        }
        5 => {
            let mut a = [[0u16; 3]; 2];
            for value in a.as_flattened_mut() {
                *value = half(r, 0x7bff);
            }
            for pixel in &mut px {
                let k = (r.next() % 2) as usize;
                *pixel = [a[k][0], a[k][1], a[k][2], 0];
            }
        }
        6 => {
            let base = u32::from(half(r, 0x7000));
            for value in px.as_flattened_mut() {
                *value = (base + r.next() % 64) as u16;
            }
        }
        _ => {
            for value in px.as_flattened_mut() {
                *value = (0x7000 + r.next() % 0xbff) as u16;
            }
        }
    }
    px.map(|pixel| [pixel[0], pixel[1], pixel[2]])
}

/// The `bc6h_pack` dump: made-up HDR pixels packed with each of the packer's five profiles, and
/// unpacked again. What `compare/dump_bc6h.cpp` prints for the same seed and count.
pub fn bc6h_pack(seed: u64, blocks: u32) -> String {
    let mut random = Random { state: seed };
    let pixels: Vec<[bc6h::RgbHalf; 16]> = (0..blocks).map(|_| make_hdr_block(&mut random)).collect();
    let mut packed = vec![[0u8; 16]; pixels.len()];
    let profiles: [(&str, Bc6hEncSettings); 5] = [("veryfast", Bc6hEncSettings::veryfast()), ("fast", Bc6hEncSettings::fast()), ("basic", Bc6hEncSettings::basic()), ("slow", Bc6hEncSettings::slow()), ("veryslow", Bc6hEncSettings::veryslow())];
    let mut modes = [0u32; 15];
    let mut printed = String::new();
    for (name, settings) in profiles {
        bc6h::compress_blocks_bc6h(&mut packed, &pixels, &settings);
        let mut hash = Hash::new();
        for block in &packed {
            hash.add(block);
            for value in bc6h::unpack_bc6h_half(block, false).as_flattened() {
                hash.add(&value.to_le_bytes());
            }
            let bits = if block[0] & 3 < 2 { block[0] & 3 } else { block[0] & 31 };
            modes[BC6H_MODE_BITS.iter().position(|&mode_bits| mode_bits == bits).unwrap_or(14)] += 1;
        }
        printed += &format!("{name} {:016x}\n", hash.value);
    }
    printed + &format!("blocks by mode:{}\n", modes.map(|count| format!(" {count}")).concat())
}

/// A whole number from -range to range, as a float.
fn spread(r: &mut Random, range: u32) -> f32 {
    ((r.next() % (2 * range + 1)) as i32 - range as i32) as f32
}

/// Sixteen made-up HDR pixels, four floats each (the fourth is not packed): noise about zero,
/// a ramp between two colours, one colour, two colours, values past what a half holds, tiny
/// values, a smooth patch, a ramp above zero.
pub fn make_float_block(r: &mut Random) -> [[f32; 4]; 16] {
    let kind = r.next() % 8;
    let (mut a, mut b, mut c) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
    for k in 0..3 {
        a[k] = spread(r, 8000) * 0.001;
        b[k] = spread(r, 8000) * 0.001;
        c[k] = spread(r, 500) * 0.001;
    }
    let mut px = [[0.0f32; 4]; 16];
    for i in 0..16 {
        px[i][3] = 1.0;
        for k in 0..3 {
            px[i][k] = match kind {
                0 => spread(r, 2000) * 0.001,
                1 => a[k] + (b[k] - a[k]) * (i as f32 * (1.0 / 15.0)) + spread(r, 20) * 0.001,
                2 => a[k],
                3 => {
                    if r.next() & 1 != 0 {
                        a[k]
                    } else {
                        b[k]
                    }
                }
                4 => spread(r, 100000),
                5 => spread(r, 1000) * 0.000001,
                6 => a[k] + (i % 4) as f32 * c[k] + (i / 4) as f32 * (c[k] * 0.5),
                _ => (if a[k] < 0.0 { -a[k] } else { a[k] }) + 0.25 + i as f32 * 0.03125,
            };
        }
    }
    px
}

/// The mode of a BC6H block, in the order the encoders try them; 14 for none.
fn bc6h_mode(block: &[u8; 16]) -> usize {
    const MODES: [i32; 32] = [0, 1, 2, 10, -1, -1, 3, 11, -1, -1, 4, 12, -1, -1, 5, 13, -1, -1, 6, -1, -1, -1, 7, -1, -1, -1, 8, -1, -1, -1, 9, -1];
    let bits = if block[0] & 2 != 0 { block[0] & 31 } else { block[0] & 3 };
    usize::try_from(MODES[usize::from(bits)]).unwrap_or(14)
}

/// The `bc6h_enc` dump: made-up pixels of eight kinds, of both signs, packed as signed and as
/// unsigned BC6H by the port of bc6h_enc.
pub fn bc6h_enc(seed: u64, blocks: u32) -> String {
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut modes = [[0u32; 15]; 2];
    let mut out = String::new();
    for i in 0..blocks {
        let pixels = make_float_block(&mut random);
        let mut block = [0xEEu8; 16];
        rustbcx::bc6h_enc::encode_bc6hs(&mut block, &pixels);
        hash.add(&block);
        modes[0][bc6h_mode(&block)] += 1;
        let mut block = [0xEEu8; 16];
        rustbcx::bc6h_enc::encode_bc6hu(&mut block, &pixels);
        hash.add(&block);
        modes[1][bc6h_mode(&block)] += 1;
        if (i + 1) % 2000 == 0 || i + 1 == blocks {
            out += &format!("bc6h_enc {} {:016x}\n", i + 1, hash.value);
        }
    }
    for (s, name) in ["signed", "unsigned"].iter().enumerate() {
        out += &format!("bc6h_enc {name} blocks by mode:");
        for count in modes[s] {
            out += &format!(" {count}");
        }
        out += "\n";
    }
    out
}

const WORDS: [&str; 16] = ["the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in", "format", "is", "cube", "data"];

/// Made-up data of one of eight kinds, the same in both programs.
pub fn make_data(r: &mut Random) -> Vec<u8> {
    let len = match r.next() % 8 {
        0 => r.next() % 16,
        1 => r.next() % 300,
        2 | 3 => r.next() % 5000,
        4 | 5 => r.next() % 40000,
        6 => 60000 + r.next() % 20000,
        _ => r.next() % 200000,
    } as usize;
    let kind = r.next() % 8;
    let mut buf = Vec::with_capacity(len);
    match kind {
        // noise
        0 => {
            for _ in 0..len {
                buf.push((r.next() >> 13) as u8);
            }
        }
        // four letters
        1 => {
            for _ in 0..len {
                buf.push(b'a' + (r.next() % 4) as u8);
            }
        }
        // words
        2 => {
            while buf.len() < len {
                let word = WORDS[(r.next() % 16) as usize];
                for &byte in word.as_bytes() {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
                if buf.len() < len {
                    buf.push(b' ');
                }
            }
        }
        // runs
        3 => {
            while buf.len() < len {
                let byte = r.next() as u8;
                let run = 1 + r.next() % 300;
                for _ in 0..run {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
            }
        }
        // copies of what came before
        4 => {
            while buf.len() < len {
                if buf.len() < 64 || r.next() % 4 == 0 {
                    buf.push((r.next() >> 9) as u8);
                } else {
                    let reach = buf.len().min(40000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 3 + r.next() % 260;
                    for _ in 0..n {
                        if buf.len() < len {
                            buf.push(buf[buf.len() - dist]);
                        }
                    }
                }
            }
        }
        // nothing
        5 => buf.resize(len, 0),
        // a slope
        6 => {
            for i in 0..len {
                buf.push((i / 7 + i % 13) as u8);
            }
        }
        // stretches of noise, of one value and of counting
        _ => {
            while buf.len() < len {
                let (stretch, what, value) = (1 + r.next() % 2000, r.next() % 3, r.next());
                for i in 0..stretch {
                    if buf.len() < len {
                        buf.push(match what {
                            0 => (r.next() >> 11) as u8,
                            1 => value as u8,
                            _ => value.wrapping_add(i) as u8,
                        });
                    }
                }
            }
        }
    }
    buf
}

fn make_tdefl_flags(r: &mut Random) -> u32 {
    use rustbcx::miniz::tdef::*;
    match r.next() % 4 {
        0 | 1 => {
            let level = (r.next() % 12) as i32 - 1;
            let strategy = if r.next() % 3 != 0 { 0 } else { (r.next() % 5) as i32 };
            let window_bits = if r.next() & 1 != 0 { 15 } else { -15 };
            let mut flags = tdefl_create_comp_flags_from_zip_params(level, window_bits, strategy);
            if r.next() & 1 != 0 {
                flags |= TDEFL_COMPUTE_ADLER32;
            }
            flags
        }
        2 => {
            let mut flags = r.next() % 4096;
            for flag in [TDEFL_WRITE_ZLIB_HEADER, TDEFL_COMPUTE_ADLER32, TDEFL_GREEDY_PARSING_FLAG, TDEFL_RLE_MATCHES, TDEFL_FILTER_MATCHES, TDEFL_FORCE_ALL_STATIC_BLOCKS, TDEFL_FORCE_ALL_RAW_BLOCKS] {
                if r.next() & 3 == 0 {
                    flags |= flag;
                }
            }
            flags
        }
        _ => {
            let mut flags = r.next() % 8;
            if r.next() & 1 != 0 {
                flags |= TDEFL_WRITE_ZLIB_HEADER;
            }
            flags
        }
    }
}

/// Decompresses with all the input and all the room at once and adds what came of it to the
/// hash: the status, and when it is done how much of the input it took and the bytes. True
/// when it is done.
fn miniz_decompress(hash: &mut Hash, stream: &[u8], room: usize, zlib: bool) -> bool {
    use rustbcx::miniz::tinfl::*;
    match tinfl_decompress(stream, room, if zlib { TINFL_FLAG_PARSE_ZLIB_HEADER } else { 0 }) {
        Ok((bytes, taken)) => {
            hash.add(&0u32.to_le_bytes());
            hash.add(&(taken as u32).to_le_bytes());
            hash.add(&(bytes.len() as u32).to_le_bytes());
            hash.add(&bytes);
            true
        }
        Err(error) => {
            let status: i32 = match error {
                TinflError::CannotMakeProgress => -4,
                TinflError::Adler32Mismatch => -2,
                TinflError::Failed => -1,
                TinflError::HasMoreOutput => 2,
            };
            hash.add(&status.to_le_bytes());
            false
        }
    }
}

/// The `miniz` dump: made-up data compressed with flags from every level and strategy and with
/// made-up flags, in one go and in pieces with flushes between; each stream decompressed
/// whole, into too little room, damaged, cut short and with bytes after it; and made-up bytes
/// decompressed.
pub fn miniz(seed: u64, count: u32) -> String {
    use rustbcx::miniz::tdef::*;
    use rustbcx::miniz::{compress, mz_adler32, mz_crc32, MZ_ADLER32_INIT, MZ_CRC32_INIT};
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut done, mut refused) = (0u32, 0u32);
    let (mut in_total, mut out_total) = (0u64, 0u64);
    let mut out = String::new();
    let mut tally = |ok: bool| if ok { done += 1 } else { refused += 1 };
    for i in 0..count {
        let data = make_data(&mut r);
        let len = data.len();
        let flags = make_tdefl_flags(&mut r);
        let zlib = flags & TDEFL_WRITE_ZLIB_HEADER != 0;
        let stream = tdefl_compress_mem_to_heap(&data, flags);
        hash.add(&(stream.len() as u32).to_le_bytes());
        hash.add(&stream);
        in_total += len as u64;
        out_total += stream.len() as u64;

        // The checksums, and what mz_compress2 writes for a level.
        hash.add(&mz_adler32(MZ_ADLER32_INIT, &data).to_le_bytes());
        hash.add(&mz_crc32(MZ_CRC32_INIT, &data).to_le_bytes());
        if i % 4 == 0 {
            let level = (r.next() % 12) as i32 - 1;
            let packed = compress(&data, level);
            hash.add(&(packed.len() as u32).to_le_bytes());
            hash.add(&packed);
        }

        // In pieces, with flushes between.
        let mut comp = TdeflCompressor::new(flags);
        let mut pos = 0;
        while pos < len {
            let n = (1 + r.next() as usize % 5000).min(len - pos);
            let how = match r.next() % 8 {
                0 => TdeflFlush::SyncFlush,
                1 => TdeflFlush::FullFlush,
                _ => TdeflFlush::NoFlush,
            };
            hash.add(&(comp.compress_buffer(&data[pos..pos + n], how) as i32).to_le_bytes());
            pos += n;
        }
        hash.add(&(comp.compress_buffer(&[], TdeflFlush::Finish) as i32).to_le_bytes());
        hash.add(&comp.adler32().to_le_bytes());
        let pieces = comp.take_output();
        hash.add(&(pieces.len() as u32).to_le_bytes());
        hash.add(&pieces);
        tally(miniz_decompress(&mut hash, &pieces, len, zlib));

        // The stream back: whole, into too little room, damaged, cut short, with more after.
        tally(miniz_decompress(&mut hash, &stream, len, zlib));
        if len > 0 {
            miniz_decompress(&mut hash, &stream, len - 1 - r.next() as usize % len, zlib);
        }
        let mut copy = stream.clone();
        for _ in 0..1 + r.next() % 3 {
            let bit = r.next() % (stream.len() as u32 * 8);
            copy[(bit >> 3) as usize] ^= 1 << (bit & 7);
        }
        tally(miniz_decompress(&mut hash, &copy, len + 64, zlib));
        miniz_decompress(&mut hash, &stream[..r.next() as usize % stream.len()], len, zlib);
        let mut copy = stream.clone();
        for _ in 0..4 {
            copy.push(r.next() as u8);
        }
        miniz_decompress(&mut hash, &copy, len, zlib);

        // Made-up bytes as a stream.
        let mut garbage: Vec<u8> = (0..1 + r.next() % 64).map(|_| (r.next() >> 7) as u8).collect();
        if r.next() & 1 != 0 {
            // (A zlib header in front, so that it gets past that.)
            garbage[0] = 0x78;
            if garbage.len() > 1 {
                garbage[1] = 0x9C;
            }
        }
        tally(miniz_decompress(&mut hash, &garbage, 4096, garbage[0] == 0x78));

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("miniz {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("miniz bytes in: {in_total}, out: {out_total}; streams decompressed: {done}, refused: {refused}\n")
}
