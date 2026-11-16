//! Made-up blocks, the same from a seed here as in `compare/dump.cpp`, and the dumps the tests
//! run as well as the comparison (`examples/dump.rs`).

// (The arithmetic is written as dump.cpp has it; not every user of this module uses all of it.)
#![allow(clippy::manual_is_multiple_of, clippy::needless_range_loop, clippy::manual_div_ceil, clippy::manual_checked_ops, dead_code)]

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

/// A made-up image of 8 bits a channel: its channels, its size, how far its rows are apart,
/// and its bytes, of one of seven kinds. What lies between the rows is 0xCD.
pub struct Picture {
    pub w: i32,
    pub h: i32,
    pub comp: i32,
    pub stride: usize,
    pub bytes: Vec<u8>,
}

fn wave(t: u32) -> u32 {
    let v = (t & 511) as i32 - 256;
    v.unsigned_abs().min(255)
}

pub fn make_picture(r: &mut Random) -> Picture {
    let comp = 1 + (r.next() % 4) as usize;
    let (w, h) = match r.next() % 6 {
        0 => (1 + r.next() % 4, 1 + r.next() % 4),
        1 | 2 => (1 + r.next() % 40, 1 + r.next() % 40),
        3 | 4 => (1 + r.next() % 100, 1 + r.next() % 100),
        _ => (1 + r.next() % 300, 1 + r.next() % 200),
    };
    let (w, h) = (w as usize, h as usize);
    let stride = w * comp + if r.next() % 3 == 0 { (r.next() % 7) as usize } else { 0 };
    let mut bytes = vec![0xCDu8; stride * h];
    let kind = r.next() % 7;
    let mut coefficient = [[0u32; 3]; 4];
    for c in &mut coefficient {
        *c = [r.next() % 64, r.next() % 64, r.next() % 4096];
    }
    let mut palette = [[0u32; 4]; 16];
    for colour in &mut palette {
        for c in colour {
            *c = r.next() & 255;
        }
    }
    let cell = 1 + r.next() % 12;
    let colours = 2 + r.next() % 4;
    let noise = 1 + r.next() % 32;
    for y in 0..h {
        for x in 0..w {
            let (ux, uy) = (x as u32, y as u32);
            let pick = if kind == 5 { r.next() % colours } else { ((ux / cell) * 7 + (uy / cell) * 13 + (ux / cell) * (uy / cell)) % 16 } as usize;
            for c in 0..comp {
                let along = ux * coefficient[c][0] + uy * coefficient[c][1] + coefficient[c][2];
                let slope = along >> 4;
                let value = match kind {
                    0 => r.next() >> 13,               // noise
                    1 => palette[0][c],                // one colour
                    2 => slope,                        // a slope
                    3 => palette[pick][c],             // squares
                    4 => slope + r.next() % noise,     // a slope with noise on it
                    5 => palette[pick][c],             // a few colours, anywhere
                    _ => wave(along),                  // waves
                };
                bytes[y * stride + x * comp + c] = value as u8;
            }
        }
    }
    Picture { w: w as i32, h: h as i32, comp: comp as i32, stride, bytes }
}

/// The `stb_image_write` dump: made-up images written as `.png` (with made-up compression
/// levels and filters), as `.bmp` and as `.jpg` (with made-up qualities), some upside down;
/// and for every fourth, made-up data through the file's own zlib compressor.
pub fn stb_image_write(seed: u64, count: u32) -> String {
    use rustbcx::stb_image_write::{write_bmp, write_jpg, write_png, zlib_compress, WriteOptions};
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut png_total, mut bmp_total, mut jpg_total, mut zlib_total, mut pixel_total) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut out = String::new();
    for i in 0..count {
        let p = make_picture(&mut r);
        pixel_total += p.w as u64 * p.h as u64;

        // The three settings.
        let options = WriteOptions {
            png_compression_level: if r.next() % 4 == 0 { (r.next() % 40) as i32 - 3 } else { 8 },
            force_png_filter: if r.next() % 3 == 0 { (r.next() % 8) as i32 - 1 } else { -1 },
            flip_vertically_on_write: r.next() % 4 == 0,
        };

        let png = write_png(&p.bytes, p.stride, p.w, p.h, p.comp, &options).expect("a png");
        hash.add(&(png.len() as u32).to_le_bytes());
        hash.add(&png);
        png_total += png.len() as u64;

        let bmp = write_bmp(p.w, p.h, p.comp, &p.bytes, p.stride, &options).expect("a bmp");
        hash.add(&(bmp.len() as u32).to_le_bytes());
        hash.add(&bmp);
        bmp_total += bmp.len() as u64;

        // Nothing for the one it stands for, in range, near where the colour stops being
        // halved, and out of range.
        let quality = match r.next() % 4 {
            0 => 0,
            1 => 1 + (r.next() % 100) as i32,
            2 => 85 + (r.next() % 16) as i32,
            _ => (r.next() % 130) as i32 - 10,
        };
        let jpg = write_jpg(p.w, p.h, p.comp, &p.bytes, quality, p.stride, &options).expect("a jpg");
        hash.add(&(jpg.len() as u32).to_le_bytes());
        hash.add(&jpg);
        jpg_total += jpg.len() as u64;

        if i % 4 == 0 {
            let data = make_data(&mut r);
            let level = if r.next() % 4 == 0 { (r.next() % 50) as i32 } else { 8 };
            let packed = zlib_compress(&data, level);
            hash.add(&(packed.len() as u32).to_le_bytes());
            hash.add(&packed);
            zlib_total += packed.len() as u64;
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("stb_image_write {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("stb_image_write pixels: {pixel_total}; bytes of .png: {png_total}, .bmp: {bmp_total}, .jpg: {jpg_total}, zlib: {zlib_total}\n")
}

fn p16be(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&(value as u16).to_be_bytes());
}

fn p32be(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&value.to_be_bytes());
}

fn p16le(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&(value as u16).to_le_bytes());
}

fn p32le(f: &mut Vec<u8>, value: u32) {
    f.extend_from_slice(&value.to_le_bytes());
}

/// A chunk of a PNG file. (No checksum: the reader does not look at it.)
fn chunk(f: &mut Vec<u8>, name: &[u8; 4], bytes: &[u8]) {
    p32be(f, bytes.len() as u32);
    f.extend_from_slice(name);
    f.extend_from_slice(bytes);
    p32be(f, 0);
}

/// A made-up PNG file: a header of any of the fifteen kinds, and made-up bytes as its rows.
pub fn make_png(r: &mut Random, f: &mut Vec<u8>) {
    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    const DEPTH: [u8; 16] = [1, 2, 4, 8, 16, 8, 16, 1, 2, 4, 8, 8, 16, 8, 16, 8];
    const COLOR: [u8; 16] = [0, 0, 0, 0, 0, 2, 2, 3, 3, 3, 3, 4, 4, 6, 6, 2];
    const CHANNELS: [u8; 7] = [1, 0, 3, 1, 2, 0, 4];
    const X_ORIG: [u32; 7] = [0, 4, 0, 2, 0, 1, 0];
    const Y_ORIG: [u32; 7] = [0, 0, 4, 0, 2, 0, 1];
    const X_SPC: [u32; 7] = [8, 8, 4, 4, 2, 2, 1];
    const Y_SPC: [u32; 7] = [8, 8, 8, 4, 4, 2, 2];
    let kind = (r.next() % 16) as usize;
    let (depth, color) = (u32::from(DEPTH[kind]), u32::from(COLOR[kind]));
    let channels = u32::from(CHANNELS[color as usize]);
    let (w, h) = if r.next() % 8 == 0 {
        let w = 1 + r.next() % 200;
        (w, 1 + r.next() % 120)
    } else {
        let w = 1 + r.next() % 40;
        (w, 1 + r.next() % 40)
    };
    let interlace = r.next() % 3 == 0;
    let style = r.next() % 3;
    let iphone = r.next() % 16 == 0;

    // The rows, each behind its filter: of the whole image, or of each of the seven passes.
    let mut raw = Vec::new();
    for pass in 0..if interlace { 7 } else { 1 } {
        let (mut pw, mut ph) = (w, h);
        if interlace {
            pw = (w.wrapping_sub(X_ORIG[pass]).wrapping_add(X_SPC[pass] - 1)) / X_SPC[pass];
            ph = (h.wrapping_sub(Y_ORIG[pass]).wrapping_add(Y_SPC[pass] - 1)) / Y_SPC[pass];
        }
        if pw == 0 || ph == 0 {
            continue;
        }
        let row_bytes = (pw * channels * depth + 7) / 8;
        for y in 0..ph {
            let mut filter = r.next() % 5;
            if r.next() % 4000 == 0 {
                filter = 5 + r.next() % 3;
            }
            raw.push(filter as u8);
            for x in 0..row_bytes {
                raw.push(match style {
                    0 => r.next() >> 13,
                    1 => r.next() % 4,
                    _ => x * 3 + y,
                } as u8);
            }
        }
    }
    // Now and then too few of them, or more than are needed.
    let how = r.next() % 40;
    if how == 0 {
        let fewer = 1 + r.next() as usize % raw.len();
        raw.truncate(raw.len() - fewer);
    } else if how == 1 {
        raw.extend_from_slice(&[0; 5]);
    }

    let z = if r.next() % 4 == 0 { rustbcx::stb_image_write::zlib_compress(&raw, 5 + (r.next() % 8) as i32) } else { rustbcx::miniz::compress(&raw, (r.next() % 10) as i32) };

    f.extend_from_slice(&SIG);
    if iphone {
        chunk(f, b"CgBI", &[0; 4]);
    }
    let mut header = Vec::new();
    p32be(&mut header, w);
    p32be(&mut header, h);
    header.extend_from_slice(&[depth as u8, color as u8, 0, 0, u8::from(interlace)]);
    chunk(f, b"IHDR", &header);
    if r.next() % 4 == 0 {
        chunk(f, b"tEXt", b"made up\0text");
    }
    // A palette: nearly always where one is needed, now and then where none is.
    let mut pal_len = 0u32;
    if if color == 3 { r.next() % 20 != 0 } else { r.next() % 8 == 0 } {
        pal_len = 1 + r.next() % 256;
        let bytes: Vec<u8> = (0..pal_len * 3).map(|_| (r.next() >> 12) as u8).collect();
        chunk(f, b"PLTE", &bytes);
    }
    // Transparency: alphas for a palette, or the one colour that is to be clear.
    if r.next() % 3 == 0 {
        let mut n = 0;
        if color == 3 {
            n = 1 + r.next() % if pal_len != 0 { pal_len } else { 1 };
            if r.next() % 30 == 0 {
                n = pal_len + 1;
            }
        } else if color == 0 {
            n = 2;
        } else if color == 2 {
            n = 6;
        } else if r.next() % 10 == 0 {
            n = 2;
        }
        if n > 256 {
            n = 256;
        }
        let bytes: Vec<u8> = (0..n).map(|_| if color == 3 { (r.next() >> 9) as u8 } else { (r.next() % 4) as u8 }).collect();
        if n != 0 {
            chunk(f, b"tRNS", &bytes);
        }
    }
    let parts = 1 + r.next() % 3;
    if r.next() % 16 == 0 {
        chunk(f, b"IDAT", &[]);
    }
    // (An iPhone file's stream has no zlib header.)
    let mut at = if iphone && z.len() > 2 { 2 } else { 0 };
    for k in 0..parts {
        let n = if k + 1 == parts { z.len() - at } else { r.next() as usize % (z.len() - at + 1) };
        chunk(f, b"IDAT", &z[at..at + n]);
        at += n;
    }
    chunk(f, b"IEND", &[]);
}

/// A made-up BMP file: a header of any of the five sizes, and made-up bytes as its rows.
pub fn make_bmp(r: &mut Random, f: &mut Vec<u8>) {
    const HSZ: [u32; 7] = [12, 40, 40, 40, 56, 108, 124];
    const BPP: [u32; 6] = [1, 4, 8, 16, 24, 32];
    let hsz = HSZ[(r.next() % 7) as usize];
    let mut bpp = BPP[(r.next() % 6) as usize];
    let mut compress = 0;
    let mut masks = [0u32; 4];
    let (mut psize, mut extra) = (0u32, 0u32);
    if r.next() % 40 == 0 {
        bpp = if r.next() % 2 != 0 { 2 } else { 20 };
    }
    if (bpp == 16 || bpp == 32) && r.next() % 2 != 0 {
        compress = 3;
    }
    if r.next() % 50 == 0 {
        compress = 1 + r.next() % 4;
    }
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let top_down = hsz != 12 && r.next() % 4 == 0;
    let mask_style = r.next() % 3;
    if mask_style == 0 {
        masks = if bpp == 16 { [0xf800, 0x07e0, 0x001f, 0] } else { [0xff0000, 0xff00, 0xff, 0xff000000] };
    } else if mask_style == 1 {
        masks = if bpp == 16 { [0x7c00, 0x03e0, 0x001f, 0x8000] } else { [0xff, 0xff00, 0xff0000, 0xff000000] };
    } else {
        for mask in &mut masks {
            let count = 1 + r.next() % 8;
            let top = if bpp == 16 { 16 } else { 32 } - count;
            let shift = r.next() % (top + 1);
            *mask = ((1u32 << count) - 1) << shift;
        }
    }
    if bpp < 16 {
        psize = 1 << bpp;
        if r.next() % 4 == 0 {
            psize = 1 + r.next() % psize;
        }
    }
    if r.next() % 10 == 0 {
        extra = 1 + r.next() % 20;
    }
    let offset = 14 + hsz + if hsz == 40 && compress == 3 { 12 } else { 0 } + psize * if hsz == 12 { 3 } else { 4 } + extra;

    f.extend_from_slice(b"BM");
    p32le(f, 0);
    p32le(f, 0);
    p32le(f, offset);
    p32le(f, hsz);
    if hsz == 12 {
        p16le(f, w);
        p16le(f, h);
        p16le(f, 1);
        p16le(f, bpp);
    } else {
        p32le(f, w);
        p32le(f, if top_down { 0u32.wrapping_sub(h) } else { h });
        p16le(f, 1);
        p16le(f, bpp);
        p32le(f, compress);
        for _ in 0..5 {
            p32le(f, 0);
        }
        if hsz == 40 && compress == 3 {
            for mask in &masks[..3] {
                p32le(f, *mask);
            }
        }
        if hsz >= 56 {
            for mask in masks {
                p32le(f, mask);
            }
        }
        if hsz >= 108 {
            for _ in 0..13 {
                p32le(f, 0);
            }
        }
        if hsz == 124 {
            for _ in 0..4 {
                p32le(f, 0);
            }
        }
    }
    for _ in 0..psize * if hsz == 12 { 3 } else { 4 } + extra {
        f.push((r.next() >> 10) as u8);
    }
    let row_bytes = ((w * bpp + 31) / 32) * 4;
    let mut pixels_size = row_bytes * h;
    // Now and then cut short.
    if r.next() % 10 == 0 {
        pixels_size -= r.next() % (pixels_size / 2 + 1);
    }
    for _ in 0..pixels_size {
        f.push((r.next() >> 13) as u8);
    }
}

/// Made-up bytes for a JPEG scan: none of them the 0xFF that begins a marker, unless followed
/// by the zero that says it is none (and, once in a while, by whatever comes next).
fn entropy_bytes(r: &mut Random, f: &mut Vec<u8>, n: u32, style: u32) {
    for _ in 0..n {
        let mut b = (r.next() >> 11) & 255;
        if style == 1 && r.next() % 4 != 0 {
            b = 0;
        }
        if style == 2 {
            b &= 0x0f;
        }
        f.push(b as u8);
        if b == 0xff && r.next() % 64 != 0 {
            f.push(0);
        }
    }
}

/// The bytes of a scan. With a restart interval, in stretches with a restart marker after
/// each: no more of them than the scan has intervals, and each short enough that the reader
/// has come to its marker by the time the interval is over.
fn entropy(r: &mut Random, f: &mut Vec<u8>, blocks: u32, mcus: u32, restart: u32) {
    let style = r.next() % 3;
    if restart != 0 {
        let segments = r.next() % ((mcus + restart - 1) / restart);
        for k in 0..segments {
            let n = r.next() % (restart / 2 + 2);
            entropy_bytes(r, f, n, style);
            f.push(0xff);
            f.push((0xd0 + (k & 7)) as u8);
        }
        let n = r.next() % (restart / 2 + 2);
        entropy_bytes(r, f, n, style);
    } else {
        let n = blocks * (1 + r.next() % 6);
        entropy_bytes(r, f, n, style);
    }
}

/// A made-up JPEG file: headers of every kind the reader takes, Huffman tables in which most
/// bits mean something, and made-up bytes as the picture.
pub fn make_jpeg(r: &mut Random, f: &mut Vec<u8>) {
    const COMPS: [u32; 5] = [1, 3, 3, 3, 4];
    const SAMPLING: [u8; 8] = [0x11, 0x22, 0x21, 0x12, 0x41, 0x22, 0x11, 0x42];
    const DC: [&[u8]; 3] = [&[0, 1, 2, 3], &[0, 2, 1, 5], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]];
    const DC_LENGTH: [u32; 3] = [2, 2, 4];
    const DC_PICK: [usize; 5] = [0, 0, 1, 1, 2];
    const AC: [&[u8]; 2] = [&[0x00, 0x01, 0x11, 0x21, 0x10, 0xF0, 0x31, 0x20], &[0x00, 0x01, 0x02, 0x03, 0x11, 0x12, 0x21, 0x31, 0x41, 0x10, 0x20, 0xF0, 0x04, 0x13, 0x51, 0x30]];
    let progressive = r.next() % 2 != 0;
    let ncomp = COMPS[(r.next() % 5) as usize];
    let w = 1 + r.next() % 64;
    let h = 1 + r.next() % 64;
    let y_sampling = u32::from(SAMPLING[(r.next() % 8) as usize]);
    let rgb_ids = r.next() % 8 == 0;
    let (mut dc_kind, mut ac_kind, mut ac_offset) = ([0usize; 2], [0u32; 2], [0u32; 2]);
    let mut restart = 0;
    for t in 0..2 {
        dc_kind[t] = DC_PICK[(r.next() % 5) as usize];
        ac_kind[t] = r.next() % 3;
        ac_offset[t] = if r.next() % 2 != 0 { 0 } else { r.next() & 255 };
    }
    if r.next() % 3 == 0 {
        restart = 1 + r.next() % 8;
    }

    p16be(f, 0xffd8);
    if r.next() % 2 != 0 {
        p16be(f, 0xffe0);
        p16be(f, 16);
        f.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    }
    if if ncomp == 4 { r.next() % 4 != 0 } else { r.next() % 3 == 0 } {
        p16be(f, 0xffee);
        p16be(f, 14);
        f.extend_from_slice(b"Adobe\0\x64\0\0\0\0");
        f.push((r.next() % 3) as u8);
    }
    if r.next() % 4 == 0 {
        let n = r.next() % 20;
        p16be(f, 0xfffe);
        p16be(f, 2 + n);
        for _ in 0..n {
            f.push((r.next() >> 10) as u8);
        }
    }

    // Two tables of divisors, of one byte a value or of two.
    let sixteen = r.next() % 6 == 0;
    let range = if r.next() % 3 == 0 { 64 } else { 4 };
    p16be(f, 0xffdb);
    p16be(f, 2 + 2 * if sixteen { 129 } else { 65 });
    for t in 0..2u8 {
        f.push((u8::from(sixteen) << 4) | t);
        for _ in 0..64 {
            let v = 1 + r.next() % range;
            if sixteen {
                p16be(f, v);
            } else {
                f.push(v as u8);
            }
        }
    }
    if restart != 0 {
        p16be(f, 0xffdd);
        p16be(f, 4);
        p16be(f, restart);
    }

    // The frame.
    f.push(0xff);
    f.push(if progressive {
        0xc2
    } else if r.next() % 4 == 0 {
        0xc1
    } else {
        0xc0
    });
    p16be(f, 8 + 3 * ncomp);
    f.push(8);
    p16be(f, h);
    p16be(f, w);
    f.push(ncomp as u8);
    let (mut id, mut hv) = ([0u32; 4], [0u32; 4]);
    for c in 0..ncomp as usize {
        id[c] = if rgb_ids && ncomp == 3 { u32::from(b"RGB"[c]) } else { c as u32 + 1 };
        hv[c] = if c == 0 || (c == 3 && r.next() % 2 != 0) { y_sampling } else { 0x11 };
        f.extend_from_slice(&[id[c] as u8, hv[c] as u8, if c == 0 { 0 } else { 1 }]);
    }
    let (hmax, vmax) = (y_sampling >> 4, y_sampling & 15);
    let mcus = ((w + 8 * hmax - 1) / (8 * hmax)) * ((h + 8 * vmax - 1) / (8 * vmax));
    let blocks = mcus * (hmax * vmax + ncomp - 1);

    // The Huffman tables: two for the first values of blocks, two for the rest.
    let mut size = 2;
    for t in 0..2 {
        size += 17 + DC[dc_kind[t]].len() as u32 + 17 + [192, 8, 16][ac_kind[t] as usize];
    }
    p16be(f, 0xffc4);
    p16be(f, size);
    for t in 0..2 {
        f.push(t as u8);
        for k in 1..=16 {
            f.push(if k == DC_LENGTH[dc_kind[t]] { DC[dc_kind[t]].len() as u8 } else { 0 });
        }
        f.extend_from_slice(DC[dc_kind[t]]);
    }
    for t in 0..2 {
        f.push(0x10 | t as u8);
        if ac_kind[t] == 0 {
            // 64 codes of seven bits and 128 of eight, which between them use every run of
            // bits, for 192 made-up symbols.
            for k in 1..=16 {
                f.push(match k {
                    7 => 64,
                    8 => 128,
                    _ => 0,
                });
            }
            for k in 0..192u32 {
                f.push((k * 167 + ac_offset[t]) as u8);
            }
        } else {
            let table = AC[ac_kind[t] as usize - 1];
            for k in 1..=16 {
                f.push(if k == if ac_kind[t] == 1 { 3 } else { 4 } { table.len() as u8 } else { 0 });
            }
            f.extend_from_slice(table);
        }
    }

    // The scans.
    let selectors = |c: usize| if c == 0 { 0x00 } else { 0x11 };
    if !progressive {
        if ncomp > 1 && r.next() % 3 == 0 {
            for c in 0..ncomp as usize {
                p16be(f, 0xffda);
                p16be(f, 8);
                f.extend_from_slice(&[1, id[c] as u8, selectors(c), 0, 63, 0]);
                entropy(r, f, blocks, mcus, restart);
                if c == 0 && r.next() % 16 == 0 {
                    let lines = if r.next() % 8 == 0 { h + 1 } else { h };
                    p16be(f, 0xffdc);
                    p16be(f, 4);
                    p16be(f, lines);
                }
            }
        } else {
            p16be(f, 0xffda);
            p16be(f, 6 + 2 * ncomp);
            f.push(ncomp as u8);
            for c in 0..ncomp as usize {
                f.extend_from_slice(&[id[c] as u8, selectors(c)]);
            }
            f.extend_from_slice(&[0, 63, 0]);
            entropy(r, f, blocks, mcus, restart);
        }
    } else {
        let scans = 1 + r.next() % 6;
        for scan in 0..scans {
            let what = if scan == 0 { 0 } else { r.next() % 4 };
            let (mut ah, mut ss, mut se, mut first, mut n) = (0, 0, 0, 0, ncomp);
            if r.next() % 3 == 0 {
                ah = 1 + r.next() % 2;
            }
            let al = if ah != 0 { ah - 1 } else { r.next() % 2 };
            if what == 0 {
                // The first values of blocks: of all the components together, or of one.
                if r.next() % 4 == 0 {
                    first = r.next() % ncomp;
                    n = 1;
                }
            } else {
                // A stretch of the rest, of one component.
                first = r.next() % ncomp;
                n = 1;
                ss = 1 + r.next() % 10;
                se = ss + r.next() % (64 - ss);
            }
            p16be(f, 0xffda);
            p16be(f, 6 + 2 * n);
            f.push(n as u8);
            for c in first as usize..(first + n) as usize {
                f.extend_from_slice(&[id[c] as u8, selectors(c)]);
            }
            f.extend_from_slice(&[ss as u8, se as u8, ((ah << 4) | al) as u8]);
            entropy(r, f, blocks, mcus, restart);
        }
    }

    // The end: now and then bytes that mean nothing before it, or no end.
    if r.next() % 10 == 0 {
        let n = r.next() % 6;
        for _ in 0..n {
            f.push(((r.next() >> 10) & 0x7f) as u8);
        }
    }
    if r.next() % 30 != 0 {
        p16be(f, 0xffd9);
    }
}

/// Adds what a read came to: the size, the channels and the values, or the reason there are
/// none. True when there were values.
fn add_result<T: Copy, const N: usize>(hash: &mut Hash, result: &Result<rustbcx::stb_image::Loaded<T>, rustbcx::stb_image::LoadError>, bytes: fn(T) -> [u8; N]) -> bool {
    match result {
        Ok(loaded) => {
            hash.add(&(loaded.x as u32).to_le_bytes());
            hash.add(&(loaded.y as u32).to_le_bytes());
            hash.add(&(loaded.channels_in_file as u32).to_le_bytes());
            for &value in &loaded.data {
                hash.add(&bytes(value));
            }
            true
        }
        Err(error) => {
            hash.add(&0xffffffffu32.to_le_bytes());
            hash.add(error.0.as_bytes());
            hash.add(&[0]);
            false
        }
    }
}

/// The `stb_image` dump: made-up `.png`, `.bmp` and `.jpg` files, written by the port of
/// stb_image_write and put together here with every kind of header and made-up bytes for the
/// picture, a third of them then damaged; each asked its size and read at 8 bits a channel,
/// every third at 16; and for every fourth, made-up data through the zlib decoder.
pub fn stb_image(seed: u64, count: u32) -> String {
    use rustbcx::stb_image::zlib::{zlib_decode_buffer, zlib_decode_malloc, zlib_decode_noheader_malloc};
    use rustbcx::stb_image::{info_from_memory, is_16_bit_from_memory, load_16_from_memory, load_from_memory, LoadOptions};
    use rustbcx::stb_image_write::{write_bmp, write_jpg, write_png, zlib_compress, WriteOptions};
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut loaded, mut refused, mut too_large) = (0u32, 0u32, 0u32);
    let mut kinds = [0u32; 4];
    let (mut pixel_total, mut file_total) = (0u64, 0u64);
    let mut out = String::new();
    for i in 0..count {
        let mut file = Vec::new();
        let kind = r.next() % 8;
        if kind < 3 {
            let p = make_picture(&mut r);
            let options = WriteOptions { png_compression_level: 5 + (r.next() % 8) as i32, force_png_filter: (r.next() % 6) as i32 - 1, flip_vertically_on_write: false };
            file = match kind {
                0 => write_png(&p.bytes, p.stride, p.w, p.h, p.comp, &options),
                1 => write_bmp(p.w, p.h, p.comp, &p.bytes, p.stride, &options),
                _ => write_jpg(p.w, p.h, p.comp, &p.bytes, 1 + (r.next() % 100) as i32, p.stride, &options),
            }
            .expect("a file");
        } else if kind < 5 {
            make_png(&mut r, &mut file);
        } else if kind == 5 {
            make_bmp(&mut r, &mut file);
        } else {
            make_jpeg(&mut r, &mut file);
        }

        // A third of them damaged: bits turned over, the end cut off, or a byte made up.
        if r.next() % 3 == 0 {
            match r.next() % 4 {
                0 => {
                    for _ in 0..1 + r.next() % 3 {
                        let bit = r.next() % (file.len() as u32 * 8);
                        file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                    }
                }
                1 => file.truncate(r.next() as usize % file.len()),
                2 => {
                    let at = r.next() % file.len() as u32;
                    file[at as usize] = r.next() as u8;
                }
                _ => {
                    let bit = r.next() % (file.len().min(64) as u32 * 8);
                    file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                }
            }
        }
        hash.add(&(file.len() as u32).to_le_bytes());
        hash.add(&file);
        file_total += file.len() as u64;

        let info = info_from_memory(&file);
        let mut large = false;
        match &info {
            Ok(info) => {
                let number = info.kind.number() as u32;
                for value in [number, info.x as u32, info.y as u32, info.comp as u32, u32::from(info.is_16_bit), u32::from(info.is_iphone_png)] {
                    hash.add(&value.to_le_bytes());
                }
                kinds[match number {
                    1 => 1,
                    2 => 2,
                    4 => 3,
                    _ => 0,
                }] += 1;
                large = u64::from(info.x as u32) * u64::from(info.y.unsigned_abs()) > 1 << 20;
            }
            Err(_) => {
                hash.add(&0u32.to_le_bytes());
                kinds[0] += 1;
            }
        }
        hash.add(&u32::from(is_16_bit_from_memory(&file)).to_le_bytes());

        let req = (r.next() % 5) as i32;
        let options = LoadOptions { flip_vertically_on_load: r.next() % 4 == 0, unpremultiply_on_load: r.next() % 2 != 0, convert_iphone_png_to_rgb: r.next() % 2 != 0, ..LoadOptions::default() };
        let req16 = (r.next() % 5) as i32;
        // (A damaged size can ask for more memory and time than there is; those are left.)
        if large {
            too_large += 1;
        } else {
            let result = load_from_memory(&file, req, &options);
            if add_result(&mut hash, &result, |value: u8| [value]) {
                loaded += 1;
                let image = result.unwrap();
                pixel_total += image.x as u64 * image.y as u64;
            } else {
                refused += 1;
            }
            if i % 3 == 0 {
                add_result(&mut hash, &load_16_from_memory(&file, req16, &options), u16::to_le_bytes);
            }
        }

        // The zlib decoder by itself.
        if i % 4 == 0 {
            let data = make_data(&mut r);
            let mut z = if r.next() % 2 != 0 { zlib_compress(&data, 5 + (r.next() % 8) as i32) } else { rustbcx::miniz::compress(&data, (r.next() % 10) as i32) };
            if r.next() % 3 == 0 {
                let bit = r.next() % (z.len() as u32 * 8);
                z[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
            match zlib_decode_malloc(&z) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(error.0.as_bytes());
                    hash.add(&[0]);
                }
            }
            // Into just enough room, or one byte too little.
            let room = data.len() - usize::from(!data.is_empty() && r.next() % 2 != 0);
            match zlib_decode_buffer(room, &z) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(&0xffffffffu32.to_le_bytes());
                    hash.add(error.0.as_bytes());
                    hash.add(&[0]);
                }
            }
            // Without its header.
            match zlib_decode_noheader_malloc(&z[2..]) {
                Ok(bytes) => {
                    hash.add(&(bytes.len() as u32).to_le_bytes());
                    hash.add(&bytes);
                }
                Err(error) => {
                    hash.add(error.0.as_bytes());
                    hash.add(&[0]);
                }
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("stb_image {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!(
        "stb_image bytes of files: {file_total}; read: {loaded} ({pixel_total} pixels), refused: {refused}, left as too large: {too_large}; found to be .jpg: {}, .png: {}, .bmp: {}, nothing: {}\n",
        kinds[1], kinds[2], kinds[3], kinds[0]
    )
}

/// Damages a third of the files: bits turned over, the end cut off, or a byte made up.
fn damage_stb_file(r: &mut Random, file: &mut Vec<u8>) {
    if r.next() % 3 == 0 {
        match r.next() % 4 {
            0 => {
                for _ in 0..1 + r.next() % 3 {
                    let bit = r.next() % (file.len() as u32 * 8);
                    file[(bit >> 3) as usize] ^= 1 << (bit & 7);
                }
            }
            1 => file.truncate(r.next() as usize % file.len()),
            2 => {
                let at = r.next() % file.len() as u32;
                file[at as usize] = r.next() as u8;
            }
            _ => {
                let bit = r.next() % (file.len().min(64) as u32 * 8);
                file[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
        }
    }
}

/// A pixel of a TGA file: an index into its palette (now and then one past its end), or bytes.
fn tga_pixel(r: &mut Random, f: &mut Vec<u8>, indexed: bool, pal_len: u32, bytes: u32) {
    if indexed {
        let index = if r.next() % 16 == 0 { r.next() % 400 } else { r.next() % pal_len };
        f.push(index as u8);
        if bytes == 2 {
            f.push((index >> 8) as u8);
        }
    } else {
        for _ in 0..bytes {
            f.push((r.next() >> 9) as u8);
        }
    }
}

/// A made-up TGA file: palette or not, grey or colour, of every depth, raw or run-length
/// encoded, either way up and round.
pub fn make_tga(r: &mut Random, f: &mut Vec<u8>) {
    const PALETTE_BITS: [u32; 5] = [8, 15, 16, 24, 32];
    const BITS: [u32; 4] = [15, 16, 24, 32];
    let indexed = r.next() % 3 == 0;
    let rle = r.next() % 2 != 0;
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let mut desc = if r.next() % 2 != 0 { 0x20u8 } else { 0 };
    if r.next() % 4 == 0 {
        desc |= 0x10;
    }
    let id_length = r.next() % 3;
    f.push(id_length as u8);
    let bytes_per_pixel;
    let mut pal_len = 0;
    if indexed {
        let pal_bits = PALETTE_BITS[(r.next() % 5) as usize];
        let index_bits = if r.next() % 4 == 0 { 16 } else { 8 };
        let pal_start = if r.next() % 4 == 0 { r.next() % 3 } else { 0 };
        let entry = if pal_bits == 15 { 2 } else { pal_bits / 8 };
        pal_len = 1 + r.next() % (if index_bits == 8 { 256 } else { 300 });
        f.push(1);
        f.push(if rle { 9 } else { 1 });
        p16le(f, pal_start);
        p16le(f, pal_len);
        f.push(pal_bits as u8);
        p16le(f, 0);
        p16le(f, 0);
        p16le(f, w);
        p16le(f, h);
        f.push(index_bits as u8);
        f.push(desc);
        for _ in 0..id_length + pal_start + pal_len * entry {
            f.push((r.next() >> 7) as u8);
        }
        bytes_per_pixel = index_bits / 8;
    } else {
        let grey = r.next() % 3 == 0;
        let bits = if grey {
            if r.next() % 2 != 0 {
                16
            } else {
                8
            }
        } else {
            BITS[(r.next() % 4) as usize]
        };
        f.push(0);
        f.push((if grey { 3 } else { 2 }) + (if rle { 8 } else { 0 }));
        p16le(f, 0);
        p16le(f, 0);
        f.push(0);
        p16le(f, 0);
        p16le(f, 0);
        p16le(f, w);
        p16le(f, h);
        f.push(bits as u8);
        f.push(desc);
        for _ in 0..id_length {
            f.push((r.next() >> 7) as u8);
        }
        bytes_per_pixel = (bits + 7) / 8;
    }
    let mut done = 0;
    while done < w * h {
        if !rle {
            tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
            done += 1;
        } else {
            let count = 1 + r.next() % 128;
            let repeat = r.next() % 2;
            f.push(((repeat << 7) | (count - 1)) as u8);
            if repeat != 0 {
                tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
            } else {
                for _ in 0..count {
                    tga_pixel(r, f, indexed, pal_len, bytes_per_pixel);
                }
            }
            done += count;
        }
    }
}

/// A made-up PSD file: none to five channels, 8 or 16 bits, raw or run-length encoded, with now
/// and then mode data, resources and reserved bytes to skip.
pub fn make_psd(r: &mut Random, f: &mut Vec<u8>) {
    let channels = r.next() % 6;
    let h = 1 + r.next() % 30;
    let w = 1 + r.next() % 30;
    let depth = if r.next() % 2 != 0 { 16 } else { 8 };
    let compression = r.next() % 2;
    let pixels = w * h;
    f.extend_from_slice(b"8BPS");
    p16be(f, if r.next() % 20 == 0 { 2 } else { 1 });
    f.extend_from_slice(&[0; 6]);
    p16be(f, channels);
    p32be(f, h);
    p32be(f, w);
    p16be(f, depth);
    p16be(f, if r.next() % 20 == 0 { 1 } else { 3 });
    for _ in 0..3 {
        let n = if r.next() % 3 == 0 { r.next() % 20 } else { 0 };
        p32be(f, n);
        for _ in 0..n {
            f.push((r.next() >> 5) as u8);
        }
    }
    p16be(f, if r.next() % 20 == 0 { 2 } else { compression });
    if compression == 0 {
        for c in 0..channels {
            for _ in 0..pixels {
                let v = if c == 3 && r.next() % 2 != 0 {
                    if r.next() % 2 != 0 {
                        0xffffffff
                    } else {
                        0
                    }
                } else {
                    r.next()
                };
                f.push((v >> 8) as u8);
                if depth == 16 {
                    f.push(v as u8);
                }
            }
        }
    } else {
        for _ in 0..h * channels {
            p16be(f, r.next() % 200);
        }
        for _ in 0..channels {
            let mut done = 0;
            while done < pixels {
                let left = pixels - done;
                let how = r.next() % 8;
                if how == 0 {
                    f.push(128);
                } else if how < 4 && left >= 2 {
                    let n = 2 + r.next() % (left.min(128) - 1);
                    f.push((257 - n) as u8);
                    f.push((r.next() >> 3) as u8);
                    done += n;
                } else {
                    let n = 1 + r.next() % left.min(128);
                    f.push((n - 1) as u8);
                    for _ in 0..n {
                        f.push((r.next() >> 3) as u8);
                    }
                    done += n;
                }
            }
        }
    }
}

/// LZW as a GIF has it, written here: the codes in blocks, and the code size as the reader
/// works it out.
struct Lzw<'a> {
    f: &'a mut Vec<u8>,
    bits: u32,
    nbits: u32,
    block_max: u32,
    block: Vec<u8>,
    mincs: u32,
    cs: u32,
    avail: u32,
    hasold: bool,
}

impl Lzw<'_> {
    fn byte(&mut self, b: u8) {
        self.block.push(b);
        if self.block.len() as u32 == self.block_max {
            self.f.push(self.block.len() as u8);
            self.f.extend_from_slice(&self.block);
            self.block.clear();
        }
    }

    fn code(&mut self, code: u32) {
        let clear = 1u32 << self.mincs;
        self.bits |= code << self.nbits;
        self.nbits += self.cs;
        while self.nbits >= 8 {
            self.byte(self.bits as u8);
            self.bits >>= 8;
            self.nbits -= 8;
        }
        if code == clear {
            self.cs = self.mincs + 1;
            self.avail = clear + 2;
            self.hasold = false;
        } else {
            if self.hasold && self.avail < 4096 {
                self.avail += 1;
                if self.avail > (1 << self.cs) - 1 && self.avail < 4096 {
                    self.cs += 1;
                }
            }
            self.hasold = true;
        }
    }
}

fn lzw_encode(f: &mut Vec<u8>, px: &[u8], mincs: u32, block_max: u32) {
    let clear = 1u32 << mincs;
    f.push(mincs as u8);
    let mut z = Lzw { f, bits: 0, nbits: 0, block_max, block: Vec::new(), mincs, cs: mincs + 1, avail: clear + 2, hasold: false };
    let mut dict = std::collections::HashMap::new();
    z.code(clear);
    let mut next = clear + 2;
    if let Some((&first, rest)) = px.split_first() {
        let mut prefix = u32::from(first);
        for &k in rest {
            let k = u32::from(k);
            if let Some(&e) = dict.get(&(prefix, k)) {
                prefix = e;
                continue;
            }
            z.code(prefix);
            if next < 4096 {
                dict.insert((prefix, k), next);
                next += 1;
            } else {
                z.code(clear);
                dict.clear();
                next = clear + 2;
            }
            prefix = k;
        }
        z.code(prefix);
    }
    z.code(clear + 1);
    if z.nbits > 0 {
        let b = z.bits as u8;
        z.byte(b);
    }
    if !z.block.is_empty() {
        z.f.push(z.block.len() as u8);
        let block = std::mem::take(&mut z.block);
        z.f.extend_from_slice(&block);
    }
    z.f.push(0);
}

/// A made-up GIF file: one to three frames, each with or without a graphic control extension
/// (a delay, a transparent colour, a way of disposing of it) and a comment, at a made-up place,
/// with a local colour table or the global one, interlaced or not.
pub fn make_gif(r: &mut Random, f: &mut Vec<u8>) {
    let big_w = 1 + r.next() % 40;
    let big_h = 1 + r.next() % 40;
    let gct = r.next() % 5 != 0;
    let gsize = r.next() % 8;
    let frames = 1 + r.next() % 3;
    f.extend_from_slice(if r.next() % 2 != 0 { b"GIF89a" } else { b"GIF87a" });
    p16le(f, big_w);
    p16le(f, big_h);
    let mut flags = (r.next() & 0x70) | gsize;
    if gct {
        flags |= 0x80;
    }
    f.push(flags as u8);
    f.push((r.next() % 256) as u8);
    f.push(0);
    if gct {
        for _ in 0..(2u32 << gsize) * 3 {
            f.push((r.next() >> 9) as u8);
        }
    }
    for _ in 0..frames {
        if r.next() % 2 != 0 {
            let mut eflags = (r.next() % 4) << 2;
            eflags |= r.next() % 2;
            f.extend_from_slice(&[0x21, 0xF9, 4, eflags as u8]);
            p16le(f, r.next() % 100);
            f.push((r.next() % 256) as u8);
            f.push(0);
        }
        if r.next() % 5 == 0 {
            let blocks = r.next() % 3;
            f.extend_from_slice(&[0x21, 0xFE]);
            for _ in 0..blocks {
                let len = 1 + r.next() % 20;
                f.push(len as u8);
                for _ in 0..len {
                    f.push((u32::from(b'a') + r.next() % 26) as u8);
                }
            }
            f.push(0);
        }
        let x = r.next() % big_w;
        let y = r.next() % big_h;
        let w = if r.next() % 20 == 0 { 0 } else { 1 + r.next() % (big_w - x) };
        let h = 1 + r.next() % (big_h - y);
        let lct = if gct { r.next() % 3 == 0 } else { r.next() % 10 != 0 };
        let lsize = r.next() % 8;
        let mut lflags = lsize;
        if lct {
            lflags |= 0x80;
        }
        if r.next() % 3 == 0 {
            lflags |= 0x40;
        }
        f.push(0x2C);
        p16le(f, x);
        p16le(f, y);
        p16le(f, w);
        p16le(f, h);
        f.push(lflags as u8);
        if lct {
            for _ in 0..(2u32 << lsize) * 3 {
                f.push((r.next() >> 9) as u8);
            }
        }
        let mut mincs = (if lct { lsize } else { gsize }) + 1;
        if mincs < 2 {
            mincs = 2;
        }
        if r.next() % 30 == 0 {
            mincs = r.next() % 14;
        }
        if mincs > 12 {
            f.push(mincs as u8);
            f.push(0);
            continue;
        }
        let colours = (1u32 << mincs).min(256);
        let n = (w * h) as usize;
        let style = r.next() % 3;
        let mut px = vec![0u8; n];
        for i in 0..n {
            px[i] = if style == 0 {
                (r.next() % colours) as u8
            } else if style == 1 {
                ((i as u32 / 7) % colours) as u8
            } else if r.next() % 4 == 0 || i == 0 {
                (r.next() % colours) as u8
            } else {
                px[i - 1]
            };
        }
        let block_max = if r.next() % 4 == 0 { 1 + r.next() % 254 } else { 255 };
        lzw_encode(f, &px, mincs, block_max);
    }
    if r.next() % 10 != 0 {
        f.push(0x3B);
    }
}

/// Whitespace between the numbers of a PNM file, now and then with a comment.
fn pnm_space(r: &mut Random, f: &mut Vec<u8>) {
    const SPACE: [u8; 6] = [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c];
    if r.next() % 6 == 0 {
        f.extend_from_slice(b"\n# made up\n");
    }
    let n = 1 + r.next() % 3;
    for _ in 0..n {
        f.push(SPACE[(r.next() % 6) as usize]);
    }
}

/// A made-up PNM file: grey or colour, binary or text, of every kind of maximum value.
pub fn make_pnm(r: &mut Random, f: &mut Vec<u8>) {
    const KINDS: [u8; 4] = *b"2356";
    let t = KINDS[(r.next() % 4) as usize];
    let ascii = t == b'2' || t == b'3';
    let comp = if t == b'3' || t == b'6' { 3 } else { 1 };
    let w = 1 + r.next() % 40;
    let h = 1 + r.next() % 40;
    let maxv = match r.next() % 8 {
        0 => 255,
        1 => 1 + r.next() % 255,
        2 => 65535,
        3 => 256 + r.next() % 65280,
        4 => 1 + r.next() % 15,
        5 => {
            if r.next() % 10 == 0 {
                0
            } else {
                255
            }
        }
        6 => {
            if r.next() % 10 == 0 {
                70000
            } else {
                1000
            }
        }
        _ => 255,
    };
    f.push(b'P');
    f.push(t);
    pnm_space(r, f);
    f.extend_from_slice(w.to_string().as_bytes());
    pnm_space(r, f);
    f.extend_from_slice(h.to_string().as_bytes());
    pnm_space(r, f);
    f.extend_from_slice(maxv.to_string().as_bytes());
    let n = w * h * comp;
    if ascii {
        for _ in 0..n {
            pnm_space(r, f);
            let v = if r.next() % 20 == 0 { r.next() % (maxv + 100) } else { r.next() % (maxv + 1) };
            f.extend_from_slice(v.to_string().as_bytes());
        }
        f.push(b'\n');
    } else {
        f.push(if r.next() % 2 != 0 { b'\n' } else { b' ' });
        for _ in 0..n {
            let v = r.next();
            if maxv > 255 {
                f.push((v >> 8) as u8);
            }
            f.push(v as u8);
        }
    }
}

/// A byte of an RGBE pixel: a colour, or an exponent near 1 (and now and then 0).
fn hdr_byte(r: &mut Random, k: u32) -> u8 {
    if k < 3 {
        return (r.next() >> 24) as u8;
    }
    if r.next() % 8 == 0 {
        0
    } else {
        (120 + r.next() % 20) as u8
    }
}

/// A made-up Radiance HDR file: a header with lines to skip, then scanlines flat or run-length
/// encoded.
pub fn make_hdr(r: &mut Random, f: &mut Vec<u8>) {
    let w = if r.next() % 2 != 0 { 1 + r.next() % 40 } else { 1 + r.next() % 300 };
    let h = 1 + r.next() % 20;
    let rle = r.next() % 4 != 0;
    f.extend_from_slice(if r.next() % 4 == 0 { b"#?RGBE\n" } else { b"#?RADIANCE\n" });
    let lines = r.next() % 3;
    for _ in 0..lines {
        f.extend_from_slice(if r.next() % 2 != 0 { b"# made up\n" } else { b"EXPOSURE=1.0\n" });
    }
    if r.next() % 30 != 0 {
        f.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    }
    if r.next() % 4 == 0 {
        f.extend_from_slice(b"GAMMA=1.0\n");
    }
    f.extend_from_slice(b"\n");
    f.extend_from_slice(if r.next() % 20 == 0 { b"+Y " } else { b"-Y " });
    f.extend_from_slice(h.to_string().as_bytes());
    f.extend_from_slice(if r.next() % 5 == 0 { b"  +X " } else { b" +X " });
    f.extend_from_slice(w.to_string().as_bytes());
    f.extend_from_slice(b"\n");
    for _ in 0..h {
        if rle && (8..32768).contains(&w) && r.next() % 20 != 0 {
            f.extend_from_slice(&[2, 2, (w >> 8) as u8, (w & 255) as u8]);
            for k in 0..4 {
                let mut x = 0;
                while x < w {
                    let left = w - x;
                    if r.next() % 2 != 0 {
                        let n = 1 + r.next() % left.min(127);
                        f.push((128 + n) as u8);
                        let value = hdr_byte(r, k);
                        f.push(value);
                        x += n;
                    } else {
                        let n = 1 + r.next() % left.min(128);
                        f.push(n as u8);
                        for _ in 0..n {
                            let value = hdr_byte(r, k);
                            f.push(value);
                        }
                        x += n;
                    }
                }
            }
        } else {
            for _ in 0..w {
                for k in 0..4 {
                    let value = hdr_byte(r, k);
                    f.push(value);
                }
            }
        }
    }
}

/// The `stb_image_more` dump: made-up `.tga`, `.psd`, `.gif`, `.pnm` and `.hdr` files, a third
/// of them damaged; each asked its size and whether it is HDR, read at 8 bits a channel, as
/// floats and every other at 16, with a made-up gamma and scale; GIFs and every fourth file
/// also read as an animation.
pub fn stb_image_more(seed: u64, count: u32) -> String {
    use rustbcx::stb_image::{info_from_memory, is_16_bit_from_memory, is_hdr_from_memory, load_16_from_memory, load_from_memory, load_gif_from_memory, loadf_from_memory, LoadOptions};
    const GAMMA: [f32; 4] = [2.2, 1.0, 1.8, 2.4];
    const SCALE: [f32; 4] = [1.0, 0.5, 2.0, 4.0];
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut loaded, mut refused, mut too_large) = (0u32, 0u32, 0u32);
    let mut kinds = [0u32; 9];
    let mut file_total = 0u64;
    let mut out = String::new();
    for i in 0..count {
        let mut file = Vec::new();
        match r.next() % 5 {
            0 => make_tga(&mut r, &mut file),
            1 => make_psd(&mut r, &mut file),
            2 => make_gif(&mut r, &mut file),
            3 => make_pnm(&mut r, &mut file),
            _ => make_hdr(&mut r, &mut file),
        }
        damage_stb_file(&mut r, &mut file);
        hash.add(&(file.len() as u32).to_le_bytes());
        hash.add(&file);
        file_total += file.len() as u64;

        let info = info_from_memory(&file);
        let mut large = false;
        let mut number = 0;
        match &info {
            Ok(info) => {
                number = info.kind.number() as u32;
                for value in [number, info.x as u32, info.y as u32, info.comp as u32, u32::from(info.is_16_bit), u32::from(info.is_hdr), u32::from(info.is_iphone_png)] {
                    hash.add(&value.to_le_bytes());
                }
                large = u64::from(info.x as u32) * u64::from(info.y.unsigned_abs()) > 1 << 20;
            }
            Err(_) => hash.add(&0u32.to_le_bytes()),
        }
        hash.add(&u32::from(is_16_bit_from_memory(&file)).to_le_bytes());
        hash.add(&u32::from(is_hdr_from_memory(&file)).to_le_bytes());
        kinds[number as usize] += 1;

        let req = (r.next() % 5) as i32;
        let flip = r.next() % 4 == 0;
        let req16 = (r.next() % 5) as i32;
        let reqf = (r.next() % 5) as i32;
        let reqg = (r.next() % 5) as i32;
        let gamma = GAMMA[(r.next() % 4) as usize];
        let scale = SCALE[(r.next() % 4) as usize];
        // (A damaged size can ask for more memory and time than there is; those are left.)
        if large {
            too_large += 1;
        } else {
            let options = LoadOptions { flip_vertically_on_load: flip, hdr_to_ldr_gamma: gamma, hdr_to_ldr_scale: scale, ldr_to_hdr_gamma: gamma, ldr_to_hdr_scale: scale, ..LoadOptions::default() };
            if add_result(&mut hash, &load_from_memory(&file, req, &options), |value: u8| [value]) {
                loaded += 1;
            } else {
                refused += 1;
            }
            if i % 2 == 0 {
                add_result(&mut hash, &load_16_from_memory(&file, req16, &options), u16::to_le_bytes);
            }
            add_result(&mut hash, &loadf_from_memory(&file, reqf, &options), f32::to_le_bytes);
            if number == 3 || i % 4 == 0 {
                match load_gif_from_memory(&file, reqg, &options) {
                    Ok(gif) => {
                        for value in [gif.x, gif.y, gif.frames, gif.channels_in_file] {
                            hash.add(&(value as u32).to_le_bytes());
                        }
                        hash.add(&gif.data);
                        for delay in gif.delays {
                            hash.add(&delay.to_le_bytes());
                        }
                    }
                    Err(error) => {
                        hash.add(&0xffffffffu32.to_le_bytes());
                        hash.add(error.0.as_bytes());
                        hash.add(&[0]);
                    }
                }
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("stb_image_more {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!(
        "stb_image_more bytes of files: {file_total}; read: {loaded}, refused: {refused}, left as too large: {too_large}; found to be .jpg: {}, .png: {}, .gif: {}, .bmp: {}, .psd: {}, .pnm: {}, .hdr: {}, .tga: {}, nothing: {}\n",
        kinds[1], kinds[2], kinds[3], kinds[4], kinds[5], kinds[6], kinds[7], kinds[8], kinds[0]
    )
}

/// The `zstd_decode` dumps: the frames `compare/dump_zstd.c` made with the zstd library and
/// wrote to a file (for each: the room to decode into, a dictionary or none, and the frame),
/// each asked its size and decoded, as that program does with the reference decoder.
pub fn zstd_decode(name: &str, count: u32, file: &[u8]) -> String {
    use rustbcx::zstd::{decompress_with_dict, get_decompressed_size, Dictionary};
    let mut hash = Hash::new();
    let (mut decoded, mut refused) = (0u32, 0u32);
    let mut frame_total = 0u64;
    let mut out = String::new();
    let mut at = 0;
    let number = |at: &mut usize| {
        let value = u32::from_le_bytes(file[*at..*at + 4].try_into().unwrap()) as usize;
        *at += 4;
        value
    };
    for i in 0..count {
        let dst_len = number(&mut at);
        let dict_size = number(&mut at);
        let dict = &file[at..at + dict_size];
        at += dict_size;
        let frame_size = number(&mut at);
        let frame = &file[at..at + frame_size];
        at += frame_size;
        frame_total += frame_size as u64;

        match get_decompressed_size(frame) {
            Ok(size) => hash.add(&(size.map_or(-2i64, |size| size as i64)).to_le_bytes()),
            Err(error) => {
                hash.add(&(-1i64).to_le_bytes());
                hash.add(error.0.as_bytes());
                hash.add(&[0]);
            }
        }
        let parsed = if dict_size != 0 { Dictionary::parse(dict) } else { Ok(Dictionary::default()) };
        match parsed.and_then(|parsed| decompress_with_dict(frame, dst_len, &parsed)) {
            Ok(bytes) => {
                hash.add(&(bytes.len() as u32).to_le_bytes());
                hash.add(&bytes);
                decoded += 1;
            }
            Err(error) => {
                hash.add(&0xffffffffu32.to_le_bytes());
                hash.add(error.0.as_bytes());
                hash.add(&[0]);
                refused += 1;
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{name} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{name} bytes of frames: {frame_total}; decoded: {decoded}, refused: {refused}\n")
}

const ZSTD_WORDS: [&str; 16] = ["the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in", "format", "is", "cube", "data"];

/// Made-up bytes of one of thirteen kinds, as `FillData` in `compare/dump_zstd_compress.c`
/// makes them.
pub fn fill_data(r: &mut Random, buf: &mut [u8], kind: u32) {
    let len = buf.len();
    let mut at = 0;
    match kind {
        // noise
        0 => {
            for byte in buf.iter_mut() {
                *byte = (r.next() >> 13) as u8;
            }
        }
        // four letters
        1 => {
            for byte in buf.iter_mut() {
                *byte = b'a' + (r.next() % 4) as u8;
            }
        }
        // words
        2 => {
            while at < len {
                let word = ZSTD_WORDS[(r.next() % 16) as usize].as_bytes();
                for &letter in word {
                    if at >= len {
                        break;
                    }
                    buf[at] = letter;
                    at += 1;
                }
                if at < len {
                    buf[at] = b' ';
                    at += 1;
                }
            }
        }
        // runs
        3 => {
            while at < len {
                let byte = r.next() as u8;
                let run = 1 + r.next() % 300;
                for _ in 0..run {
                    if at >= len {
                        break;
                    }
                    buf[at] = byte;
                    at += 1;
                }
            }
        }
        // copies of what came before
        4 => {
            while at < len {
                let pick = if at < 64 { 0 } else { r.next() % 4 };
                if pick == 0 {
                    buf[at] = (r.next() >> 9) as u8;
                    at += 1;
                } else {
                    let reach = at.min(40000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 3 + r.next() % 260;
                    for _ in 0..n {
                        if at >= len {
                            break;
                        }
                        buf[at] = buf[at - dist];
                        at += 1;
                    }
                }
            }
        }
        // nothing
        5 => buf.fill(0),
        // a slope
        6 => {
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = (i / 7 + i % 13) as u8;
            }
        }
        // stretches of noise, of one value and of counting
        7 => {
            while at < len {
                let stretch = 1 + r.next() % 2000;
                let what = r.next() % 3;
                let value = r.next();
                for i in 0..stretch {
                    if at >= len {
                        break;
                    }
                    buf[at] = if what == 0 {
                        (r.next() >> 11) as u8
                    } else if what == 1 {
                        value as u8
                    } else {
                        value.wrapping_add(i) as u8
                    };
                    at += 1;
                }
            }
        }
        // parts of the kinds above, one after the other
        8 => {
            while at < len {
                let mut n = 1 + r.next() as usize % 200000;
                let part = r.next() % 8;
                if n > len - at {
                    n = len - at;
                }
                fill_data(r, &mut buf[at..at + n], part);
                at += n;
            }
        }
        // the same few bytes over and over, changed in a few places
        9 => {
            let period = 1 + r.next() as usize % 5000;
            for i in 0..period.min(len) {
                buf[i] = (r.next() >> 11) as u8;
            }
            for i in period..len {
                buf[i] = buf[i - period];
            }
            let changes = r.next() % 6;
            if len != 0 {
                for _ in 0..changes {
                    let place = r.next() as usize % len;
                    buf[place] ^= (1 + r.next() % 255) as u8;
                }
            }
        }
        // a few low values
        11 => {
            let m = 2 + r.next() % 14;
            for byte in buf.iter_mut() {
                *byte = (r.next() % m) as u8;
            }
        }
        // words of four bytes of noise, in any order
        12 => {
            let words = 1000 + r.next() as usize % 3000;
            for i in 0..(words * 4).min(len) {
                buf[i] = (r.next() >> 12) as u8;
            }
            at = words * 4;
            while at < len {
                let word = r.next() as usize % words * 4;
                for i in 0..4 {
                    if at >= len {
                        break;
                    }
                    buf[at] = buf[word + i];
                    at += 1;
                }
            }
        }
        // sixty-four letters, with a rare copy of what came before
        _ => {
            while at < len {
                let pick = if at > 1000 { r.next() % 30000 } else { 1 };
                if pick == 0 {
                    let reach = at.min(100000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 8 + r.next() % 300;
                    for _ in 0..n {
                        if at >= len {
                            break;
                        }
                        buf[at] = buf[at - dist];
                        at += 1;
                    }
                } else {
                    buf[at] = (48 + (r.next() >> 10) % 64) as u8;
                    at += 1;
                }
            }
        }
    }
}

const ZSTD_LEVELS: [i32; 20] = [1, 1, 2, 2, 3, 3, 3, 0, 4, 4, -1, -1, -2, -3, -5, -7, -20, -100, -1000, -200000];

/// The `zstd_compress` dumps: made-up data compressed at the levels that are ported, as
/// `compare/dump_zstd_compress.c` does with `ZSTD_compress`. With `check`, each frame is
/// also decoded by the port of the reference decoder and compared with its data.
pub fn zstd_compress(name: &str, seed: u64, count: u32, check: bool) -> String {
    zstd_compress_up_to(name, seed, count, check, 9)
}

/// The `zstd_compress` dumps, and the `zstd_compress_more` ones (levels 4 to 22), where a
/// level whose strategy for the size is past `highest` is taken as level 3.
pub fn zstd_compress_up_to(name: &str, seed: u64, count: u32, check: bool, highest: u32) -> String {
    const MAX_DATA: u32 = 5000000;
    let small = name == "zstd_compress_small";
    let more = name.starts_with("zstd_compress_more");
    let more_small = name == "zstd_compress_more_small";
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut in_total, mut frame_total) = (0u64, 0u64);
    let mut out = String::new();
    let mut data = Vec::new();
    for i in 0..count {
        let kind = r.next() % 13;
        let size = r.next() % 16;
        let mut max = match size {
            0 => 16,
            1..=4 => 300,
            5..=8 => 5000,
            9..=11 => 40000,
            12..=13 => 300000,
            14 => 1500000,
            _ => MAX_DATA,
        };
        if small && max > 300000 {
            max = 300000;
        }
        if more_small && max > 40000 {
            max = 40000;
        }
        let mut len = (r.next() % (max + 1)) as usize;
        let mut level;
        if more {
            level = 4 + (r.next() % 19) as i32;
            if r.next().is_multiple_of(20) {
                level = 23 + (r.next() % 5) as i32;
            }
            if rustbcx::zstd::compare_hooks::strategy_of(level, len.max(1)) > highest {
                level = 3;
            }
        } else {
            level = ZSTD_LEVELS[(r.next() % 20) as usize];
        }
        // A block of nothing but matches of four bytes has so many sequences that their
        // number is written the long way: it takes level 3 and a source of exactly this size.
        if kind == 12 && r.next() % 2 == 0 {
            len = 262144;
            level = 3;
        }
        // Level 4 takes a strategy that is ported for these sizes only.
        if !more && level == 4 && !(len > 256 * 1024 || (len > 16 * 1024 && len <= 128 * 1024)) {
            level = 3;
        }
        data.clear();
        data.resize(len, 0);
        fill_data(&mut r, &mut data, kind);
        let frame = rustbcx::zstd::compress(&data, level).expect("a level that is ported");
        if check {
            assert_eq!(rustbcx::zstd::decompress(&frame, len).as_deref(), Ok(&data[..]), "frame {i} does not give its data back");
        }
        hash.add(&(len as u32).to_le_bytes());
        hash.add(&(level as u32).to_le_bytes());
        hash.add(&(frame.len() as u32).to_le_bytes());
        hash.add(&frame);
        if std::env::var_os("SHOW_EACH").is_some() {
            eprintln!("frame {i}: {len} bytes at level {level}, {} bytes, {:016x}", frame.len(), hash.value);
        }
        in_total += len as u64;
        frame_total += frame.len() as u64;
        if (i + 1) % 10 == 0 || i + 1 == count {
            out.push_str(&format!("{name} {} {:016x}\n", i + 1, hash.value));
        }
    }
    out.push_str(&format!("{name} bytes: {in_total}; bytes of frames: {frame_total}\n"));
    out
}

/// The `zstd_compress_long` dump: made-up data of 64 to 80 megabytes, as `FillLong` in
/// `compare/dump_zstd_compress.c` makes it, compressed at level 22, which takes
/// long-distance matching for so much.
pub fn zstd_compress_long(name: &str, seed: u64, count: u32, check: bool) -> String {
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let (mut in_total, mut frame_total) = (0u64, 0u64);
    let mut out = String::new();
    for i in 0..count {
        let len = 64 * 1024 * 1024 + (r.next() % 16000000) as usize;
        let level = 22;
        let mut data = vec![0u8; len];
        let mut at = 0;
        while at < len {
            let piece = (1 + (r.next() % 4000000) as usize).min(len - at);
            if at > 1000000 && r.next().is_multiple_of(2) {
                let dist = 1 + r.next() as usize % at.min(70000000);
                let changes = r.next() % 16;
                for i in at..at + piece {
                    data[i] = data[i - dist];
                }
                for _ in 0..changes {
                    let place = at + r.next() as usize % piece;
                    data[place] = r.next() as u8;
                }
            } else {
                let kind = r.next() % 13;
                fill_data(&mut r, &mut data[at..at + piece], kind);
            }
            at += piece;
        }
        let frame = rustbcx::zstd::compress(&data, level).expect("every level is ported");
        if check {
            assert_eq!(rustbcx::zstd::decompress(&frame, len).as_deref(), Ok(&data[..]), "frame {i} does not give its data back");
        }
        hash.add(&(len as u32).to_le_bytes());
        hash.add(&(level as u32).to_le_bytes());
        hash.add(&(frame.len() as u32).to_le_bytes());
        hash.add(&frame);
        in_total += len as u64;
        frame_total += frame.len() as u64;
        out.push_str(&format!("{name} {} {:016x}\n", i + 1, hash.value));
    }
    out.push_str(&format!("{name} bytes: {in_total}; bytes of frames: {frame_total}\n"));
    out
}

/// Made-up literals of one of six kinds, as `MakeLiterals` in `compare/dump_zstd_entropy.c`
/// makes them.
fn make_literals(r: &mut Random, buf: &mut [u8]) {
    let len = buf.len();
    let shape = r.next() % 6;
    match shape {
        // one value
        0 => {
            let value = r.next();
            buf.fill(value as u8);
        }
        // a few low values
        1 => {
            let m = 2 + r.next() % 15;
            for byte in buf.iter_mut() {
                *byte = (r.next() % m) as u8;
            }
        }
        // values with few bits set more often
        2 => {
            for byte in buf.iter_mut() {
                let value = r.next();
                *byte = (value & (value >> 8) & (value >> 16)) as u8;
            }
        }
        // noise
        3 => {
            for byte in buf.iter_mut() {
                *byte = (r.next() >> 13) as u8;
            }
        }
        // letters and spaces
        4 => {
            for byte in buf.iter_mut() {
                let value = r.next();
                *byte = if value % 7 == 0 { b' ' } else { b'a' + ((value >> 8) % 26) as u8 };
            }
        }
        // a few low values, then noise
        _ => {
            let m = 2 + r.next() % 15;
            for (i, byte) in buf.iter_mut().enumerate() {
                let value = r.next();
                *byte = if i < len / 2 { (value % m) as u8 } else { (value >> 13) as u8 };
            }
        }
    }
}

/// An answer as a number: a size, or an error's code with the top bit set.
fn entropy_code(answer: &Result<usize, u32>) -> u32 {
    match answer {
        Ok(size) => *size as u32,
        Err(code) => 0x80000000 + code,
    }
}

/// What the `zstd_entropy` dump keeps from one round to the next.
struct Entropy {
    r: Random,
    hash: Hash,
    outputs: u32,
    errors: u32,
    lit: Vec<u8>,
    out: Vec<u8>,
    huf: rustbcx::zstd::compare_hooks::Huf,
    fresh: bool,
    literals: rustbcx::zstd::compare_hooks::Literals,
}

impl Entropy {
    fn number(&mut self, value: u32) {
        self.hash.add(&value.to_le_bytes());
    }

    fn try_fse(&mut self) {
        use rustbcx::zstd::compare_hooks as hooks;
        let mut count = [0u32; 256];
        let mut norm = [0i16; 256];
        let mut header = [0u8; 512];
        let mut pool = [0u8; 4096];
        let mut symbols = [0u8; 600];
        let r = &mut self.r;
        let max = if r.next() % 4 == 0 { 1 + r.next() % 255 } else { 1 + r.next() % 52 };
        let shape = r.next() % 8;
        let big = 1 + r.next() % 5000;
        for s in 0..=max {
            count[s as usize] = match shape {
                0 => 1 + r.next() % 3,
                1 => {
                    if s < 64 {
                        big >> (s / 2)
                    } else {
                        0
                    }
                }
                2 => {
                    let c = r.next() % 5;
                    if c == 0 {
                        1 + r.next() % 40
                    } else {
                        0
                    }
                }
                3 => {
                    if s == 0 {
                        big * 20
                    } else {
                        r.next() % 3
                    }
                }
                4 => big % 4 + 1,
                5 => r.next() % 1000,
                6 => {
                    if s < 3 {
                        big * 30
                    } else if s < 6 {
                        big * 3
                    } else {
                        1 + r.next() % 2
                    }
                }
                _ => u32::from(r.next() % 16 < 11),
            };
        }
        if count[max as usize] == 0 {
            count[max as usize] = 1;
        }
        let mut total: usize = count[..=max as usize].iter().map(|&c| c as usize).sum();
        if total < 2 {
            count[0] += 2;
            total += 2;
        }
        let mut table_log = if r.next() % 4 == 0 { r.next() % 14 } else { hooks::fse_optimal_table_log(5 + r.next() % 8, total, max) };
        let low = r.next() & 1 != 0;

        let answer = hooks::fse_normalize_count(&mut norm, table_log, &count, total, max, low);
        self.number(entropy_code(&answer.map(|log| log as usize)));
        match answer {
            Ok(log) if log != 0 => table_log = log,
            _ => {
                self.errors += 1;
                return;
            }
        }
        for s in 0..=max as usize {
            self.number(norm[s] as i32 as u32);
        }

        let mut room = 512;
        if self.r.next() % 3 == 0 {
            room = 2 + self.r.next() % 22;
        }
        let answer = hooks::fse_write_ncount(&mut header[..room as usize], &norm, max, table_log);
        self.number(entropy_code(&answer));
        match answer {
            Ok(size) => self.hash.add(&header[..size]),
            Err(_) => {
                self.errors += 1;
                return;
            }
        }

        let mut p = 0;
        for s in 0..=max as usize {
            for _ in 0..norm[s].unsigned_abs() {
                pool[p] = s as u8;
                p += 1;
            }
        }
        let n = self.r.next() % 600;
        for symbol in symbols.iter_mut().take(n as usize) {
            *symbol = pool[self.r.next() as usize % p];
        }
        let mut room = n + (n >> 7) + 12 + self.r.next() % 64;
        if self.r.next() % 3 == 0 {
            room = self.r.next() % (n + 16);
        }
        let size = hooks::fse_compress(&mut self.out[..room as usize], &symbols[..n as usize], &norm, max, table_log);
        self.number(size as u32);
        self.hash.add(&self.out[..size]);
        self.outputs += 1;
    }

    fn try_huf(&mut self) {
        let mut again = false;
        if self.fresh {
            again = self.r.next() % 4 == 0;
        }
        let mut repeat;
        if again {
            repeat = if self.r.next() % 2 != 0 { 2 } else { 1 };
        } else {
            let size = self.r.next() % 8;
            let pick = self.r.next();
            let len = match size {
                0 => pick % 70,
                1..=2 => 40 + pick % 300,
                3..=4 => pick % 3000,
                5 => 900 + pick % 300,
                6 => 16000 + pick % 1000,
                _ => pick % 131200,
            };
            self.lit.clear();
            self.lit.resize(len as usize, 0);
            make_literals(&mut self.r, &mut self.lit);
            repeat = if self.r.next() % 3 == 0 { 1 } else { 0 };
        }
        let four = self.r.next() & 1 != 0;
        let prefer = self.r.next() & 1 != 0;
        let suspect = self.r.next() & 1 != 0;
        let len = self.lit.len();
        let mut room = len + (len >> 8) + 200;
        if self.r.next() % 3 == 0 {
            room = self.r.next() as usize % (len + 32);
        }
        let answer = self.huf.compress(&mut self.out[..room], &self.lit, four, &mut repeat, prefer, suspect);
        self.number(entropy_code(&answer));
        self.number(repeat);
        match answer {
            Ok(size) => {
                self.hash.add(&self.out[..size]);
                self.outputs += 1;
            }
            Err(_) => self.errors += 1,
        }
        // A new table was made and used: it has a code for every byte of these literals.
        self.fresh = matches!(answer, Ok(size) if size > 1) && repeat == 0;
    }

    fn try_literals(&mut self) {
        let size = self.r.next() % 6;
        let pick = self.r.next();
        let len = match size {
            0 => pick % 70,
            1 => 60 + pick % 10,
            2 => pick % 2000,
            3 => 1000 + pick % 50,
            4 => 16370 + pick % 30,
            _ => pick % 70000,
        } as usize;
        // The literals of the Huffman rounds are gone: no going over them again.
        self.fresh = false;
        self.lit.clear();
        self.lit.resize(len, 0);
        make_literals(&mut self.r, &mut self.lit);
        let strategy = 1 + self.r.next() % 2;
        let disable = self.r.next() % 8 == 0;
        let suspect = self.r.next() & 1 != 0;
        let mut room = len + 300;
        if self.r.next() % 3 == 0 {
            room = self.r.next() as usize % (len + 16);
        }
        let answer = self.literals.compress(&mut self.out[..room], &self.lit, strategy, disable, suspect);
        self.number(entropy_code(&answer));
        self.number(self.literals.next_repeat_mode());
        match answer {
            Ok(size) => {
                self.hash.add(&self.out[..size]);
                self.outputs += 1;
                // As a block that is kept does: the tables of this one are the next one's.
                if self.r.next() % 2 != 0 {
                    self.literals.keep();
                }
            }
            Err(_) => self.errors += 1,
        }
    }
}

/// The `zstd_entropy` dump: the entropy coders of the Zstandard compressor one function at
/// a time, as `compare/dump_zstd_entropy.c` asks the originals.
pub fn zstd_entropy(seed: u64, count: u32) -> String {
    let name = "zstd_entropy";
    let mut e = Entropy { r: Random { state: seed }, hash: Hash::new(), outputs: 0, errors: 0, lit: Vec::new(), out: vec![0u8; 150000], huf: Default::default(), fresh: false, literals: Default::default() };
    let mut out = String::new();
    for i in 0..count {
        let what = e.r.next() % 20;
        if what < 12 {
            e.try_fse();
        } else if what < 17 {
            e.try_huf();
        } else {
            e.try_literals();
        }
        if (i + 1) % 1000 == 0 || i + 1 == count {
            out.push_str(&format!("{name} {} {:016x}\n", i + 1, e.hash.value));
        }
    }
    out.push_str(&format!("{name} outputs: {}, refused: {}\n", e.outputs, e.errors));
    out
}

/// Whether the comparison leaves a file with this header unread. A damaged size can ask for
/// more memory and time than there is; those are left, but for the sizes tinyexr refuses at
/// once. Two more are left, which stop tinyexr: a tile size of 0, which it divides by, and a
/// count of chunks in the thousands of millions, which it makes a table for before it looks
/// whether the file has one.
pub fn tinyexr_left(header: &rustbcx::tinyexr::ExrHeader) -> bool {
    let limit = 1024 * 8192;
    let w = i64::from(header.data_window.max_x) - i64::from(header.data_window.min_x) + 1;
    let h = i64::from(header.data_window.max_y) - i64::from(header.data_window.min_y) + 1;
    let (tile_x, tile_y) = (i64::from(header.tile_size_x), i64::from(header.tile_size_y));
    let refused = w <= 0 || h <= 0 || w > limit || h > limit || (header.tiled != 0 && (tile_x > limit || tile_y > limit));
    if refused {
        false
    } else if header.tiled != 0 {
        w * h > (1 << 22) || tile_x <= 0 || tile_y <= 0 || tile_x * tile_y > (1 << 16) || (w / tile_x + 1) * (h / tile_y + 1) * tile_x * tile_y > (1 << 23)
    } else {
        w * h > (1 << 22) || header.chunk_count > (1 << 24)
    }
}

/// The `tinyexr_read` dumps: the files `compare/dump_tinyexr.cc` made with tinyexr and wrote
/// to a file (for each: whether halves are to be read as floats, and the file), each read as
/// that program reads it with tinyexr: its version, its header, its image, and it as floats
/// of red, green, blue and alpha.
pub fn tinyexr_read(name: &str, count: u32, file: &[u8]) -> String {
    use rustbcx::tinyexr::{self, ChannelData, ExrError, ExrImage};
    let mut hash = Hash::new();
    let (mut headers, mut images, mut refused, mut left, mut as_rgba, mut stopped) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut file_total = 0u64;
    let mut out = String::new();
    let mut at = 0;
    let number = |at: &mut usize| {
        let value = u32::from_le_bytes(file[*at..*at + 4].try_into().unwrap()) as usize;
        *at += 4;
        value
    };
    fn add_number(hash: &mut Hash, value: i32) {
        hash.add(&value.to_le_bytes());
    }
    // A name with its end, or a mark in its place where it was not UTF-8.
    fn add_name(hash: &mut Hash, text: &str) {
        if text.contains('\u{fffd}') {
            hash.add(&0xfffffffdu32.to_le_bytes());
        } else {
            hash.add(text.as_bytes());
            hash.add(&[0]);
        }
    }
    fn add_message(hash: &mut Hash, message: &str) {
        if message.is_empty() {
            hash.add(&0xfffffffeu32.to_le_bytes());
        } else {
            hash.add(message.as_bytes());
            hash.add(&[0]);
        }
    }
    fn add_error(hash: &mut Hash, error: &ExrError) {
        add_number(hash, error.code);
        add_message(hash, &error.message);
    }
    fn add_values(hash: &mut Hash, values: &ChannelData) {
        match values {
            ChannelData::Uint(values) => values.iter().for_each(|value| hash.add(&value.to_le_bytes())),
            ChannelData::Half(values) => values.iter().for_each(|value| hash.add(&value.to_le_bytes())),
            ChannelData::Float(values) => values.iter().for_each(|value| hash.add(&value.to_bits().to_le_bytes())),
        }
    }
    // The file as floats of red, green, blue and alpha.
    fn continue_with_rgba(hash: &mut Hash, wish: usize, bytes: &[u8], as_rgba: &mut u32) {
        let loaded = tinyexr::load_exr_from_memory_as_tinyexr(bytes);
        if wish & 8 != 0 {
            hash.add(&0xfffffffcu32.to_le_bytes());
            if loaded.is_ok() {
                hash.add(b"read where tinyexr stops");
            }
            return;
        }
        match loaded {
            Err(error) => add_error(hash, &error),
            Ok((rgba, width, height)) => {
                add_number(hash, 0);
                add_message(hash, "");
                *as_rgba += 1;
                add_number(hash, width);
                add_number(hash, height);
                for value in &rgba {
                    hash.add(&value.to_bits().to_le_bytes());
                }
            }
        }
    }
    for i in 0..count {
        let wish = number(&mut at);
        let size = number(&mut at);
        let bytes = &file[at..at + size];
        at += size;
        file_total += size as u64;

        add_number(&mut hash, size as i32);
        hash.add(bytes);

        let mut large = false;
        match tinyexr::parse_exr_version_from_memory(bytes) {
            Err(code) => add_number(&mut hash, code),
            Ok(version) => {
                add_number(&mut hash, 0);
                add_number(&mut hash, version.version);
                add_number(&mut hash, i32::from(version.tiled));
                add_number(&mut hash, i32::from(version.long_name));
                add_number(&mut hash, i32::from(version.non_image));
                add_number(&mut hash, i32::from(version.multipart));

                match tinyexr::parse_exr_header_from_memory_as_tinyexr(&version, bytes) {
                    Err(error) => add_error(&mut hash, &error),
                    Ok(mut header) => {
                        add_number(&mut hash, 0);
                        add_message(&mut hash, "");
                        headers += 1;
                        hash.add(&header.pixel_aspect_ratio.to_bits().to_le_bytes());
                        add_number(&mut hash, header.line_order);
                        for window in [header.data_window, header.display_window] {
                            add_number(&mut hash, window.min_x);
                            add_number(&mut hash, window.min_y);
                            add_number(&mut hash, window.max_x);
                            add_number(&mut hash, window.max_y);
                        }
                        hash.add(&header.screen_window_center[0].to_bits().to_le_bytes());
                        hash.add(&header.screen_window_center[1].to_bits().to_le_bytes());
                        hash.add(&header.screen_window_width.to_bits().to_le_bytes());
                        add_number(&mut hash, header.chunk_count);
                        add_number(&mut hash, header.tiled);
                        add_number(&mut hash, header.tile_size_x);
                        add_number(&mut hash, header.tile_size_y);
                        add_number(&mut hash, header.tile_level_mode);
                        add_number(&mut hash, header.tile_rounding_mode);
                        add_number(&mut hash, header.long_name);
                        add_number(&mut hash, header.non_image);
                        add_number(&mut hash, header.multipart);
                        add_number(&mut hash, header.header_len as i32);
                        add_number(&mut hash, header.custom_attributes.len() as i32);
                        for attribute in &header.custom_attributes {
                            add_name(&mut hash, &attribute.name);
                            add_name(&mut hash, &attribute.type_name);
                            add_number(&mut hash, attribute.value.len() as i32);
                            hash.add(&attribute.value);
                        }
                        add_number(&mut hash, header.channels.len() as i32);
                        for (c, channel) in header.channels.iter().enumerate() {
                            add_name(&mut hash, &channel.name);
                            add_number(&mut hash, channel.pixel_type);
                            add_number(&mut hash, channel.x_sampling);
                            add_number(&mut hash, channel.y_sampling);
                            add_number(&mut hash, i32::from(channel.p_linear));
                            add_number(&mut hash, header.pixel_types[c]);
                            add_number(&mut hash, header.requested_pixel_types[c]);
                        }
                        add_number(&mut hash, header.compression_type);
                        add_name(&mut hash, &header.name);

                        large = tinyexr_left(&header);
                        if large {
                            left += 1;
                        } else {
                            // (A damaged file that is not compressed is read with halves as
                            // floats: tinyexr reads its halves as halves without a look at
                            // the length of a block, past the end of the file.)
                            if wish & 1 != 0 || (wish & 2 != 0 && header.compression_type == tinyexr::COMPRESSIONTYPE_NONE) {
                                for c in 0..header.channels.len() {
                                    if header.pixel_types[c] == tinyexr::PIXELTYPE_HALF {
                                        header.requested_pixel_types[c] = tinyexr::PIXELTYPE_FLOAT;
                                    }
                                }
                            }
                            let loaded = tinyexr::load_exr_image_from_memory_as_tinyexr(&mut header, bytes);
                            match (wish & 4 != 0, loaded) {
                                // Where tinyexr stops (it throws, and nothing catches it),
                                // the port has to refuse the file.
                                (true, loaded) => {
                                    hash.add(&0xfffffffcu32.to_le_bytes());
                                    stopped += 1;
                                    if loaded.is_ok() {
                                        hash.add(b"read where tinyexr stops");
                                    }
                                }
                                (false, Err(error)) => {
                                    add_error(&mut hash, &error);
                                    refused += 1;
                                }
                                (false, Ok(image)) => {
                                    add_number(&mut hash, 0);
                                    add_message(&mut hash, "");
                                    images += 1;
                                    for &pixel_type in &header.pixel_types {
                                        add_number(&mut hash, pixel_type);
                                    }
                                    let mut next: Option<&ExrImage> = Some(&image);
                                    while let Some(level) = next {
                                        add_number(&mut hash, level.level_x);
                                        add_number(&mut hash, level.level_y);
                                        add_number(&mut hash, level.width);
                                        add_number(&mut hash, level.height);
                                        add_number(&mut hash, level.num_channels);
                                        add_number(&mut hash, level.tiles.len() as i32);
                                        if !level.tiles.is_empty() {
                                            for tile in &level.tiles {
                                                add_number(&mut hash, tile.offset_x);
                                                add_number(&mut hash, tile.offset_y);
                                                add_number(&mut hash, tile.level_x);
                                                add_number(&mut hash, tile.level_y);
                                                add_number(&mut hash, tile.width);
                                                add_number(&mut hash, tile.height);
                                                for values in &tile.images {
                                                    add_values(&mut hash, values);
                                                }
                                            }
                                        } else {
                                            for values in &level.images {
                                                add_values(&mut hash, values);
                                            }
                                        }
                                        next = level.next_level.as_deref();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !large {
            continue_with_rgba(&mut hash, wish, bytes, &mut as_rgba);
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{name} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{name} bytes of files: {file_total}; headers read: {headers}, images read: {images}, refused: {refused}, left: {left}, where tinyexr stops: {stopped}; as red, green, blue and alpha: {as_rgba}\n")
}

/// A made-up image and how to save it, as `MakeImage` in `compare/dump_tinyexr.cc` makes it.
struct ExrMade {
    header: rustbcx::tinyexr::ExrHeader,
    image: rustbcx::tinyexr::ExrImage,
    /// Whether what is saved of it has to be read back: not with attributes that say other
    /// than the image is, and not with a level left out.
    plain: bool,
}

/// The values of one channel, `w` by `h` of them, of one of four kinds: noise, a ramp, one
/// value, values such as a picture has. (`MakeValues`.)
fn exr_values(r: &mut Random, pixel_type: i32, w: i32, h: i32) -> rustbcx::tinyexr::ChannelData {
    use rustbcx::tinyexr::{ChannelData, PIXELTYPE_FLOAT, PIXELTYPE_HALF};
    let kind = r.next() % 4;
    let first = r.next();
    let step_x = r.next() % 8;
    let step_y = r.next() % 8;
    let mut values = Vec::with_capacity((w * h) as usize);
    for y in 0..h as u32 {
        for x in 0..w as u32 {
            values.push(match kind {
                0 => r.next(),
                1 => first.wrapping_add(x.wrapping_mul(step_x)).wrapping_add(y.wrapping_mul(step_y)),
                2 => first,
                _ => {
                    let n = r.next();
                    if pixel_type == PIXELTYPE_FLOAT {
                        0x3c000000 + n % 0x04000000
                    } else if pixel_type == PIXELTYPE_HALF {
                        0x3000 + n % 0x1000
                    } else {
                        n % 1000
                    }
                }
            });
        }
    }
    if pixel_type == PIXELTYPE_FLOAT {
        ChannelData::Float(values.into_iter().map(f32::from_bits).collect())
    } else if pixel_type == PIXELTYPE_HALF {
        ChannelData::Half(values.into_iter().map(|value| value as u16).collect())
    } else {
        ChannelData::Uint(values)
    }
}

/// A double such as the settings of ZFP have: a rate, a tolerance, or one that is not either.
/// (`ZfpDouble`: less than nothing, which asks for most of four gigabits a block, only where
/// there is more than one channel, `many`.)
fn exr_zfp_double(r: &mut Random, tolerance: bool, many: bool) -> f64 {
    const RATES: [f64; 16] = [0.5, 1.0, 2.0, 2.0, 4.0, 7.3, 8.0, 12.0, 16.0, 24.0, 31.9, 32.0, 40.0, 0.0, -0.01, 1e-9];
    const TOLERANCES: [f64; 16] = [1e-3, 0.5, 1e-7, 0.01, 2.0, 1e-30, 0.0, -1.0, 1e-40, 1e30, 1e-320, 0.25, 3.0, 1e-5, 100.0, 1e300];
    let kind = r.next() % 32;
    if kind < 16 {
        return if tolerance { TOLERANCES[kind as usize] } else { RATES[kind as usize] };
    }
    if kind < 28 {
        return f64::from(r.next() % 400) / if tolerance { 1000.0 } else { 10.0 };
    }
    match kind {
        28 => f64::NAN,
        29 => f64::INFINITY,
        30 if many => -1.0,
        _ => 100.0,
    }
}

/// A made-up image (`MakeImage`); for ZFP compression (`g_zfp`), of floats, most of them of
/// whole blocks of four by four, with the attributes of ZFP's settings.
fn exr_image(r: &mut Random, small: bool, zfp: bool) -> ExrMade {
    use rustbcx::tinyexr::{self, Attribute, Box2i, ChannelInfo, ExrHeader, ExrImage, ExrTile};
    const NAMES: [&str; 8] = ["A", "B", "G", "R", "Z", "id", "light.B", "light.G"];
    let mut plain = true;
    let tiled = r.next().is_multiple_of(3);
    let size_kind = if zfp {
        8 + r.next() % 8
    } else if small {
        0
    } else {
        r.next() % 8
    };
    let (width, height);
    if size_kind >= 8 {
        // (For ZFP: of whole blocks, now and then not.)
        if size_kind < 15 {
            width = 4 * (1 + (r.next() % 12) as i32);
            height = 4 * (1 + (r.next() % 12) as i32);
        } else {
            width = 1 + (r.next() % 40) as i32;
            height = 1 + (r.next() % 40) as i32;
        }
    } else if size_kind < 5 {
        width = 1 + (r.next() % if small { 10 } else { 40 }) as i32;
        height = 1 + (r.next() % if small { 36 } else { 40 }) as i32;
    } else if size_kind < 7 {
        width = 1 + (r.next() % 150) as i32;
        height = 1 + (r.next() % 150) as i32;
    } else {
        width = 1 + (r.next() % 400) as i32;
        height = 1 + (r.next() % 100) as i32;
    }

    // Its channels: red, green, blue and alpha; without alpha; one channel; or some of eight.
    let (count, start);
    let names_kind = r.next() % 4;
    if names_kind == 0 {
        count = 4;
        start = 0;
    } else if names_kind == 1 {
        count = 3;
        start = 1;
    } else if names_kind == 2 {
        count = 1;
        start = 0;
    } else {
        count = 1 + r.next() % 5;
        start = r.next() % (8 - count + 1);
    }
    let mut header = ExrHeader::default();
    for c in 0..count {
        let name = if names_kind == 2 { "Y" } else { NAMES[(start + c) as usize] };
        if zfp {
            // Floats, halves or integers handed over, stored as floats, now and then not.
            let kind = r.next() % 16;
            let memory_type = if kind < 10 {
                tinyexr::PIXELTYPE_FLOAT
            } else if kind < 15 {
                tinyexr::PIXELTYPE_HALF
            } else {
                tinyexr::PIXELTYPE_UINT
            };
            let as_half = r.next().is_multiple_of(24);
            let file_type = if as_half && memory_type != tinyexr::PIXELTYPE_UINT { tinyexr::PIXELTYPE_HALF } else { tinyexr::PIXELTYPE_FLOAT };
            header.channels.push(ChannelInfo { name: name.to_string(), pixel_type: file_type, x_sampling: 1, y_sampling: 1, p_linear: 0 });
            header.pixel_types.push(memory_type);
            header.requested_pixel_types.push(file_type);
            continue;
        }
        let memory_type = (r.next() % 3) as i32;
        let as_half = r.next() % 2;
        let file_type = if memory_type == tinyexr::PIXELTYPE_UINT {
            tinyexr::PIXELTYPE_UINT
        } else if as_half != 0 {
            tinyexr::PIXELTYPE_HALF
        } else {
            tinyexr::PIXELTYPE_FLOAT
        };
        header.channels.push(ChannelInfo { name: name.to_string(), pixel_type: file_type, x_sampling: 1, y_sampling: 1, p_linear: 0 });
        header.pixel_types.push(memory_type);
        header.requested_pixel_types.push(file_type);
    }

    // Its compression: one of the eight, PIZ more often; now and then one that is not written.
    let compression = r.next() % 20;
    header.compression_type = if compression < 16 {
        (compression % 8) as i32
    } else if compression == 16 {
        tinyexr::COMPRESSIONTYPE_ZFP
    } else if compression == 17 {
        9
    } else {
        tinyexr::COMPRESSIONTYPE_PIZ
    };
    if zfp {
        header.compression_type = tinyexr::COMPRESSIONTYPE_ZFP;
    } else if header.compression_type == tinyexr::COMPRESSIONTYPE_ZFP {
        // (A ZFP file without the attributes of its settings, as these other images are, is
        // written, at 2 bits a value, and not read.)
        plain = false;
    }
    header.long_name = i32::from(r.next().is_multiple_of(8));
    if zfp {
        // The settings: a rate, a precision or a tolerance, each with its type; now and then
        // a type it does not know, none, a type of other than one byte, or no value for it.
        let kind = r.next() % 32;
        let zfp_type = if kind < 12 {
            0
        } else if kind < 18 {
            1
        } else if kind < 28 {
            2
        } else if kind == 28 {
            3 + r.next() % 253
        } else {
            0
        };
        let attribute = |name: &str, type_name: &str, value: Vec<u8>| Attribute { name: name.to_string(), type_name: type_name.to_string(), value };
        if kind != 29 {
            header.custom_attributes.push(attribute("zfpCompressionType", "uchar", vec![zfp_type as u8; if kind == 30 { 2 } else { 1 }]));
        }
        if kind != 31 {
            if zfp_type == 0 {
                let rate = exr_zfp_double(r, false, header.channels.len() > 1);
                header.custom_attributes.push(attribute("zfpCompressionRate", "double", rate.to_le_bytes().to_vec()));
            } else if zfp_type == 1 {
                let precision = r.next() % 70;
                header.custom_attributes.push(attribute("zfpCompressionPrecision", "int", precision.to_le_bytes().to_vec()));
            } else {
                let tolerance = exr_zfp_double(r, true, header.channels.len() > 1);
                header.custom_attributes.push(attribute("zfpCompressionTolerance", "double", tolerance.to_le_bytes().to_vec()));
            }
        }
        // (A file without them saves, at 2 bits a value, and is not read.)
        if kind >= 28 {
            plain = false;
        }
    }
    // Now and then attributes of its own.
    let mut with_chunk_count = false;
    let mut wrong_chunk_count = 0u32;
    if !zfp && r.next().is_multiple_of(3) {
        let attributes = 1 + r.next() % 3;
        for j in 0..attributes {
            let kind = r.next() % 16;
            let mut attribute = Attribute { name: String::new(), type_name: "string".to_string(), value: Vec::new() };
            if kind < 6 {
                attribute.name = if kind < 4 { "comment" } else { "owner" }.to_string();
                attribute.value = vec![0; (r.next() % 20) as usize];
                for byte in &mut attribute.value {
                    *byte = b'a' + (r.next() % 26) as u8;
                }
            } else if kind < 10 {
                const TYPES: [&str; 5] = ["scanlineimage", "tiledimage", "deeptile", "deepscanline", "other"];
                attribute.name = "type".to_string();
                attribute.value = TYPES[(r.next() % 5) as usize].as_bytes().to_vec();
                plain = false;
            } else if kind < 12 {
                attribute.name = "name".to_string();
                attribute.value = vec![0; 1 + (r.next() % 8) as usize];
                for byte in &mut attribute.value {
                    *byte = b'a' + (r.next() % 26) as u8;
                }
            } else if kind < 15 {
                // (Its value is filled in once the image is made.)
                let wrong = r.next() % 4;
                let count = r.next() % 40;
                if with_chunk_count {
                    continue;
                }
                with_chunk_count = true;
                wrong_chunk_count = if wrong == 0 { count + 1 } else { 0 };
                attribute.name = "chunkCount".to_string();
                attribute.type_name = "int".to_string();
                attribute.value = vec![0; 4];
            } else {
                let many = 126 + r.next() % 6;
                attribute.type_name = "int".to_string();
                attribute.value = vec![j as u8; 4];
                for k in 0..many {
                    attribute.name = format!("a{k:03}");
                    header.custom_attributes.push(attribute.clone());
                }
                continue;
            }
            header.custom_attributes.push(attribute);
        }
    }
    // The count of its chunks into the attribute for it, if it has one: the count, or another.
    let mut set_chunk_count = |header: &mut ExrHeader, count: u32| {
        for attribute in &mut header.custom_attributes {
            if attribute.name == "chunkCount" {
                let mut value = count;
                if wrong_chunk_count != 0 {
                    let other = wrong_chunk_count - 1;
                    value = if other >= count { other + 1 } else { other };
                }
                attribute.value = value.to_le_bytes().to_vec();
                if value != count {
                    plain = false;
                }
            }
        }
    };

    let mut image = ExrImage { width, height, num_channels: count as i32, ..ExrImage::default() };
    header.data_window = Box2i { min_x: 0, min_y: 0, max_x: width - 1, max_y: height - 1 };
    if !tiled {
        for c in 0..count as usize {
            image.images.push(exr_values(r, header.pixel_types[c], width, height));
        }
        let lines = match header.compression_type {
            tinyexr::COMPRESSIONTYPE_ZIP | tinyexr::COMPRESSIONTYPE_PXR24 | tinyexr::COMPRESSIONTYPE_ZFP => 16,
            tinyexr::COMPRESSIONTYPE_PIZ | tinyexr::COMPRESSIONTYPE_B44 | tinyexr::COMPRESSIONTYPE_B44A => 32,
            _ => 1,
        };
        set_chunk_count(&mut header, ((height + lines - 1) / lines) as u32);
        return ExrMade { header, image, plain };
    }

    header.tiled = 1;
    header.tile_size_x = 1 + (r.next() % width.min(64) as u32) as i32;
    header.tile_size_y = 1 + (r.next() % height.min(64) as u32) as i32;
    if zfp && !r.next().is_multiple_of(8) {
        // (Of whole blocks, and of one level, most of them.)
        header.tile_size_x = (header.tile_size_x + 3) & !3;
        header.tile_size_y = (header.tile_size_y + 3) & !3;
    }
    header.tile_level_mode = if zfp && !r.next().is_multiple_of(4) { tinyexr::TILE_ONE_LEVEL } else { (r.next() % 3) as i32 };
    header.tile_rounding_mode = (r.next() % 2) as i32;
    let without_last_level = r.next().is_multiple_of(16);

    // Its levels, as tinyexr counts them.
    let levels = tinyexr::tile_levels(&header).expect("the levels of a made-up image");
    let mut images = Vec::new();
    let mut chunks = 0u32;
    for level in &levels {
        let mut level_image = ExrImage { level_x: level.level_x, level_y: level.level_y, width: level.width, height: level.height, num_channels: count as i32, ..ExrImage::default() };
        for t in 0..level.num_x_tiles * level.num_y_tiles {
            let (offset_x, offset_y) = (t % level.num_x_tiles, t / level.num_x_tiles);
            let mut tile = ExrTile {
                offset_x,
                offset_y,
                level_x: level.level_x,
                level_y: level.level_y,
                width: header.tile_size_x.min(level.width - offset_x * header.tile_size_x),
                height: header.tile_size_y.min(level.height - offset_y * header.tile_size_y),
                images: Vec::new(),
            };
            for c in 0..count as usize {
                tile.images.push(exr_values(r, header.pixel_types[c], header.tile_size_x, header.tile_size_y));
            }
            level_image.tiles.push(tile);
            chunks += 1;
        }
        images.push(level_image);
    }
    set_chunk_count(&mut header, chunks);
    if without_last_level && images.len() > 1 {
        images.pop();
        plain = false;
    }
    // The levels one behind the other.
    let mut chain: Option<Box<ExrImage>> = None;
    while let Some(mut level) = images.pop() {
        level.next_level = chain.take();
        chain = Some(Box::new(level));
    }
    ExrMade { header, image: *chain.expect("a level"), plain }
}

/// The `tinyexr_write` dumps: made-up images saved, as `compare/dump_tinyexr.cc` saves them
/// with tinyexr: a hash of each file's bytes, or of the message where none is saved. Each
/// file is read back besides, which the other program does not do: what was saved as it was
/// has to come back as it was.
pub fn tinyexr_write(name: &str, seed: u64, count: u32) -> String {
    use rustbcx::tinyexr::{self, ChannelData};
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let small = name.ends_with("_small");
    let mut file_total = 0u64;
    let (mut saved, mut not_saved, mut short_way) = (0u32, 0u32, 0u32);
    let mut out = String::new();
    // The values of a channel as bits, to compare them.
    fn bits(values: &ChannelData, index: usize) -> u32 {
        match values {
            ChannelData::Uint(values) => values[index],
            ChannelData::Half(values) => u32::from(values[index]),
            ChannelData::Float(values) => values[index].to_bits(),
        }
    }
    for i in 0..count {
        let made = exr_image(&mut r, small, name.starts_with("tinyexr_zfp"));
        match tinyexr::save_exr_image_to_memory_as_tinyexr(&made.image, &made.header) {
            Err(error) => {
                not_saved += 1;
                // (What tinyexr gives as the size: nothing, or one of its error numbers.)
                let refusal = if error.code == tinyexr::ERROR_SERIALIZATION_FAILED { 0 } else { error.code };
                hash.add(&refusal.to_le_bytes());
                hash.add(error.message.as_bytes());
                hash.add(&[0]);
            }
            Ok(file) => {
                saved += 1;
                file_total += file.len() as u64;
                hash.add(&(file.len() as u32).to_le_bytes());
                hash.add(&file);

                if made.plain {
                    // Read back from the file as the format has it, which is tinyexr's own
                    // unless a PXR24 or B44 block did not come out smaller than its pixels.
                    let lossy_blocks = matches!(made.header.compression_type, tinyexr::COMPRESSIONTYPE_PXR24 | tinyexr::COMPRESSIONTYPE_B44 | tinyexr::COMPRESSIONTYPE_B44A);
                    let file = if lossy_blocks { tinyexr::save_exr_image_to_memory(&made.image, &made.header).unwrap_or_else(|error| panic!("{name}: file {i} is not saved the other way: {error:?}")) } else { file };
                    let version = tinyexr::parse_exr_version_from_memory(&file).unwrap_or_else(|code| panic!("{name}: file {i} is not read back: {code}"));
                    let mut header = tinyexr::parse_exr_header_from_memory(&version, &file).unwrap_or_else(|error| panic!("{name}: file {i} is not read back: {error:?}"));
                    let read = tinyexr::load_exr_image_from_memory(&mut header, &file).unwrap_or_else(|error| panic!("{name}: file {i} is not read back: {error:?}"));
                    // A channel stored as the type it had, by a compression that loses
                    // nothing of that type, comes back bit for bit.
                    let comp = made.header.compression_type;
                    let (mut ours, mut theirs) = (Some(&made.image), Some(&read));
                    while let (Some(wrote), Some(got)) = (ours, theirs) {
                        assert_eq!((wrote.width, wrote.height, wrote.tiles.len()), (got.width, got.height, got.tiles.len()), "{name}: file {i}");
                        for c in 0..made.header.channels.len() {
                            let file_type = made.header.requested_pixel_types[c];
                            let kept = made.header.pixel_types[c] == file_type
                                && comp != tinyexr::COMPRESSIONTYPE_ZFP
                                && !(comp == tinyexr::COMPRESSIONTYPE_PXR24 && file_type == tinyexr::PIXELTYPE_FLOAT)
                                && !((comp == tinyexr::COMPRESSIONTYPE_B44 || comp == tinyexr::COMPRESSIONTYPE_B44A) && file_type == tinyexr::PIXELTYPE_HALF);
                            if !kept {
                                continue;
                            }
                            if wrote.tiles.is_empty() {
                                for index in 0..(wrote.width * wrote.height) as usize {
                                    assert_eq!(bits(&wrote.images[c], index), bits(&got.images[c], index), "{name}: file {i}, channel {c}, value {index}");
                                }
                            }
                            for (tile, got_tile) in wrote.tiles.iter().zip(&got.tiles) {
                                assert_eq!((tile.width, tile.height, tile.offset_x, tile.offset_y), (got_tile.width, got_tile.height, got_tile.offset_x, got_tile.offset_y), "{name}: file {i}");
                                for y in 0..tile.height as usize {
                                    for x in 0..tile.width as usize {
                                        let index = y * made.header.tile_size_x as usize + x;
                                        assert_eq!(bits(&tile.images[c], index), bits(&got_tile.images[c], index), "{name}: file {i}, channel {c}, a tile's value {index}");
                                    }
                                }
                            }
                        }
                        ours = wrote.next_level.as_deref();
                        theirs = got.next_level.as_deref();
                    }
                    assert!(ours.is_none() && theirs.is_none(), "{name}: file {i} comes back with other levels");
                }
            }
        }

        // Every fourth, floats saved the short way: one, three or four to a pixel (or two,
        // which it does not take), as halves or as floats.
        if i % 4 == 0 {
            const COMPONENTS: [i32; 8] = [2, 1, 1, 3, 3, 3, 4, 4];
            let components = COMPONENTS[(r.next() % 8) as usize];
            let width = 1 + (r.next() % 40) as i32;
            let height = 1 + (r.next() % 40) as i32;
            let as_halves = r.next() % 2 != 0;
            let ChannelData::Float(floats) = exr_values(&mut r, tinyexr::PIXELTYPE_FLOAT, width * components, height) else { unreachable!() };
            match tinyexr::save_exr_to_memory(&floats, width, height, components, as_halves) {
                Ok(file) => {
                    short_way += 1;
                    file_total += file.len() as u64;
                    hash.add(&(file.len() as u32).to_le_bytes());
                    hash.add(&file);
                    // And back: the floats, or the halves of them.
                    let (rgba, read_width, read_height) = tinyexr::load_exr_from_memory(&file).unwrap_or_else(|error| panic!("{name}: file {i}, saved the short way, is not read back: {error:?}"));
                    assert_eq!((read_width, read_height), (width, height));
                    if !as_halves {
                        for pixel in 0..(width * height) as usize {
                            for k in 0..components as usize {
                                // (One channel comes back as all four.)
                                let wrote = floats[pixel * components as usize + k];
                                assert_eq!(wrote.to_bits(), rgba[pixel * 4 + k].to_bits(), "{name}: file {i}, saved the short way, pixel {pixel}");
                            }
                        }
                    }
                }
                Err(error) => {
                    hash.add(&error.code.to_le_bytes());
                    hash.add(error.message.as_bytes());
                    hash.add(&[0]);
                }
            }
        }

        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{name} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{name} bytes of files: {file_total}; saved: {saved}, not saved: {not_saved}, saved the short way: {short_way}\n")
}

/// A number into a hash, as `AddNumber` in `compare/dump_tinyexr.cc` puts it.
fn exr_add_number(hash: &mut Hash, value: i32) {
    hash.add(&value.to_le_bytes());
}

/// A name with its end, or a mark in its place where it was not UTF-8 (`AddName`).
fn exr_add_name(hash: &mut Hash, text: &str) {
    if text.contains('\u{fffd}') {
        hash.add(&0xfffffffdu32.to_le_bytes());
    } else {
        hash.add(text.as_bytes());
        hash.add(&[0]);
    }
}

/// An error's number and message, or a mark where it has none (`AddMessage2`).
fn exr_add_error(hash: &mut Hash, code: i32, message: &str) {
    exr_add_number(hash, code);
    if message.is_empty() {
        hash.add(&0xfffffffeu32.to_le_bytes());
    } else {
        hash.add(message.as_bytes());
        hash.add(&[0]);
    }
}

/// A header, as `AddHeader` puts it.
fn exr_add_header(hash: &mut Hash, header: &rustbcx::tinyexr::ExrHeader) {
    hash.add(&header.pixel_aspect_ratio.to_bits().to_le_bytes());
    exr_add_number(hash, header.line_order);
    for window in [header.data_window, header.display_window] {
        exr_add_number(hash, window.min_x);
        exr_add_number(hash, window.min_y);
        exr_add_number(hash, window.max_x);
        exr_add_number(hash, window.max_y);
    }
    hash.add(&header.screen_window_center[0].to_bits().to_le_bytes());
    hash.add(&header.screen_window_center[1].to_bits().to_le_bytes());
    hash.add(&header.screen_window_width.to_bits().to_le_bytes());
    exr_add_number(hash, header.chunk_count);
    exr_add_number(hash, header.tiled);
    exr_add_number(hash, header.tile_size_x);
    exr_add_number(hash, header.tile_size_y);
    exr_add_number(hash, header.tile_level_mode);
    exr_add_number(hash, header.tile_rounding_mode);
    exr_add_number(hash, header.long_name);
    exr_add_number(hash, header.non_image);
    exr_add_number(hash, header.multipart);
    exr_add_number(hash, header.header_len as i32);
    exr_add_number(hash, header.custom_attributes.len() as i32);
    for attribute in &header.custom_attributes {
        exr_add_name(hash, &attribute.name);
        exr_add_name(hash, &attribute.type_name);
        exr_add_number(hash, attribute.value.len() as i32);
        hash.add(&attribute.value);
    }
    exr_add_number(hash, header.channels.len() as i32);
    for (c, channel) in header.channels.iter().enumerate() {
        exr_add_name(hash, &channel.name);
        exr_add_number(hash, channel.pixel_type);
        exr_add_number(hash, channel.x_sampling);
        exr_add_number(hash, channel.y_sampling);
        exr_add_number(hash, i32::from(channel.p_linear));
        exr_add_number(hash, header.pixel_types[c]);
        exr_add_number(hash, header.requested_pixel_types[c]);
    }
    exr_add_number(hash, header.compression_type);
    exr_add_name(hash, &header.name);
}

/// An image and its levels, as `AddImage` puts them.
fn exr_add_image(hash: &mut Hash, image: &rustbcx::tinyexr::ExrImage, header: &rustbcx::tinyexr::ExrHeader) {
    use rustbcx::tinyexr::ChannelData;
    fn add_values(hash: &mut Hash, values: &ChannelData) {
        match values {
            ChannelData::Uint(values) => values.iter().for_each(|value| hash.add(&value.to_le_bytes())),
            ChannelData::Half(values) => values.iter().for_each(|value| hash.add(&value.to_le_bytes())),
            ChannelData::Float(values) => values.iter().for_each(|value| hash.add(&value.to_bits().to_le_bytes())),
        }
    }
    for &pixel_type in &header.pixel_types {
        exr_add_number(hash, pixel_type);
    }
    let mut next = Some(image);
    while let Some(level) = next {
        exr_add_number(hash, level.level_x);
        exr_add_number(hash, level.level_y);
        exr_add_number(hash, level.width);
        exr_add_number(hash, level.height);
        exr_add_number(hash, level.num_channels);
        exr_add_number(hash, level.tiles.len() as i32);
        if !level.tiles.is_empty() {
            for tile in &level.tiles {
                exr_add_number(hash, tile.offset_x);
                exr_add_number(hash, tile.offset_y);
                exr_add_number(hash, tile.level_x);
                exr_add_number(hash, tile.level_y);
                exr_add_number(hash, tile.width);
                exr_add_number(hash, tile.height);
                for values in &tile.images {
                    add_values(hash, values);
                }
            }
        } else {
            for values in &level.images {
                add_values(hash, values);
            }
        }
        next = level.next_level.as_deref();
    }
}

/// Into the hash, what came of a reading, as `AddOutcome` puts it: the flags (1 where the
/// original's answer depends on what is outside the file, which the port refuses with
/// `PAST_THE_END`; 2 where it stops, which the port refuses, with that or another error), or
/// 0 and the hash of what was read. `theirs` is what the other program found; the flags.
fn exr_add_outcome(hash: &mut Hash, end: ExrEnd, read: &Hash, theirs: u32) -> u32 {
    let refused = end == ExrEnd::PastTheEnd || (theirs == 2 && end == ExrEnd::Error);
    let flags = if refused { if theirs != 0 { theirs } else { 1 } } else { 0 };
    hash.add(&flags.to_le_bytes());
    if flags == 0 {
        hash.add(&read.value.to_le_bytes());
    }
    flags
}

/// How a reading ended.
#[derive(Clone, Copy, PartialEq)]
enum ExrEnd {
    /// With what was read, or with the file left.
    Read,
    /// With an error.
    Error,
    /// Refused with `PAST_THE_END`.
    PastTheEnd,
}

/// The reading of a file of many parts, as `ReadParts` does it.
fn exr_read_parts(hash: &mut Hash, bytes: &[u8], wish: u32) -> ExrEnd {
    use rustbcx::tinyexr::{self, PAST_THE_END};
    let version = match tinyexr::parse_exr_version_from_memory(bytes) {
        Err(code) => {
            exr_add_number(hash, code);
            return ExrEnd::Error;
        }
        Ok(version) => version,
    };
    exr_add_number(hash, 0);
    exr_add_number(hash, version.version);
    exr_add_number(hash, i32::from(version.tiled));
    exr_add_number(hash, i32::from(version.long_name));
    exr_add_number(hash, i32::from(version.non_image));
    exr_add_number(hash, i32::from(version.multipart));
    let mut headers = match tinyexr::parse_exr_multipart_header_from_memory_as_tinyexr(&version, bytes) {
        Err(error) if error.message == PAST_THE_END => return ExrEnd::PastTheEnd,
        Err(error) => {
            exr_add_error(hash, error.code, &error.message);
            return ExrEnd::Error;
        }
        Ok(headers) => headers,
    };
    exr_add_error(hash, 0, "");
    exr_add_number(hash, headers.len() as i32);
    let mut left = headers.len() > 64;
    let mut chunks = 0i64;
    for header in &headers {
        exr_add_header(hash, header);
        left |= tinyexr_left(header);
        chunks += i64::from(header.chunk_count.max(0));
    }
    if left || chunks > 1 << 16 {
        hash.add(&0xfffffffbu32.to_le_bytes());
        return ExrEnd::Read;
    }
    for (i, header) in headers.iter_mut().enumerate() {
        if i < 32 && (wish >> i) & 1 != 0 {
            for c in 0..header.channels.len() {
                if header.pixel_types[c] == tinyexr::PIXELTYPE_HALF {
                    header.requested_pixel_types[c] = tinyexr::PIXELTYPE_FLOAT;
                }
            }
        }
    }
    match tinyexr::load_exr_multipart_image_from_memory_as_tinyexr(&mut headers, bytes) {
        Err(error) if error.message == PAST_THE_END => ExrEnd::PastTheEnd,
        Err(error) => {
            exr_add_error(hash, error.code, &error.message);
            ExrEnd::Error
        }
        Ok(images) => {
            exr_add_error(hash, 0, "");
            for (image, header) in images.iter().zip(&headers) {
                exr_add_image(hash, image, header);
            }
            ExrEnd::Read
        }
    }
}

/// The `tinyexr_parts` dump: files of many parts made from the seed and saved, as
/// `compare/dump_tinyexr.cc` saves them with tinyexr (the bytes, or the message); and the
/// files that program wrote to the file of files (each with what is to be read of it as
/// floats, and what came of it there), read as it reads them.
pub fn tinyexr_parts(name: &str, seed: u64, count: u32, file: &[u8]) -> String {
    use rustbcx::tinyexr;
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let mut file_total = 0u64;
    let (mut saved, mut not_saved, mut past_end, mut stopped) = (0u32, 0u32, 0u32, 0u32);
    let mut out = String::new();
    let mut at = 0;
    let number = |at: &mut usize| {
        let value = u32::from_le_bytes(file[*at..*at + 4].try_into().unwrap());
        *at += 4;
        value
    };
    for i in 0..count {
        let num_parts = if r.next().is_multiple_of(10) { 1 } else { 2 + r.next() % 3 };
        let mut parts = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for p in 0..num_parts {
            parts.push(exr_image(&mut r, true, false));
            let kind = r.next() % 40;
            let part_name = if kind == 0 {
                String::new()
            } else if kind == 1 && p > 0 {
                names[names.len() - 1].clone()
            } else {
                format!("part{p}")
            };
            names.push(part_name);
        }
        for (made, part_name) in parts.iter_mut().zip(&names) {
            made.header.name = part_name.clone();
        }
        let list: Vec<_> = parts.iter().map(|made| (&made.image, &made.header)).collect();
        match tinyexr::save_exr_multipart_image_to_memory_as_tinyexr(&list) {
            Err(error) => {
                not_saved += 1;
                let refusal = if error.code == tinyexr::ERROR_SERIALIZATION_FAILED { 0 } else { error.code };
                hash.add(&refusal.to_le_bytes());
                hash.add(error.message.as_bytes());
                hash.add(&[0]);
            }
            Ok(saved_file) => {
                saved += 1;
                file_total += saved_file.len() as u64;
                hash.add(&(saved_file.len() as u32).to_le_bytes());
                hash.add(&saved_file);
            }
        }

        let wish = number(&mut at);
        let theirs = number(&mut at);
        let size = number(&mut at) as usize;
        let bytes = &file[at..at + size];
        at += size;
        let mut read = Hash::new();
        let end = exr_read_parts(&mut read, bytes, wish);
        let flags = exr_add_outcome(&mut hash, end, &read, theirs);
        past_end += u32::from(flags == 1);
        stopped += u32::from(flags == 2);

        if std::env::var_os("SHOW_EACH").is_some() {
            eprintln!("file {i} {:016x}", hash.value);
        }
        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{name} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{name} bytes of files: {file_total}; saved: {saved}, not saved: {not_saved}; read where it depends on what is past the end: {past_end}, where tinyexr stops: {stopped}\n")
}

/// The `tinyexr_deep` dump: the deep files `compare/dump_tinyexr.cc` made and wrote to the
/// file of files (each with what came of it there), read as it reads them.
pub fn tinyexr_deep(name: &str, count: u32, file: &[u8]) -> String {
    use rustbcx::tinyexr::{self, PAST_THE_END};
    let mut hash = Hash::new();
    let mut file_total = 0u64;
    let (mut past_end, mut stopped, mut left) = (0u32, 0u32, 0u32);
    let mut out = String::new();
    let mut at = 0;
    let number = |at: &mut usize| {
        let value = u32::from_le_bytes(file[*at..*at + 4].try_into().unwrap());
        *at += 4;
        value
    };
    for i in 0..count {
        let theirs = number(&mut at);
        let size = number(&mut at) as usize;
        let bytes = &file[at..at + size];
        at += size;
        file_total += size as u64;
        exr_add_number(&mut hash, size as i32);
        hash.add(bytes);

        if theirs == 4 {
            // (Left unread there: more pixels than there is time and memory for.)
            left += 1;
            hash.add(&4u32.to_le_bytes());
        } else {
            let mut read = Hash::new();
            let end = match tinyexr::load_deep_exr_from_memory(bytes) {
                Err(error) if error.message == PAST_THE_END => ExrEnd::PastTheEnd,
                Err(error) => {
                    exr_add_error(&mut read, error.code, &error.message);
                    ExrEnd::Error
                }
                Ok(deep) => {
                    exr_add_error(&mut read, 0, "");
                    exr_add_number(&mut read, deep.width);
                    exr_add_number(&mut read, deep.height);
                    exr_add_number(&mut read, deep.num_channels);
                    for channel_name in &deep.channel_names {
                        exr_add_name(&mut read, channel_name);
                    }
                    if deep.width > 0 && deep.height > 0 {
                        for row in &deep.offset_table {
                            for &offset in row {
                                exr_add_number(&mut read, offset);
                            }
                        }
                        for rows in &deep.image {
                            for (y, row) in rows.iter().enumerate() {
                                exr_add_number(&mut read, y as i32);
                                let n = deep.offset_table[y][deep.width as usize - 1];
                                if n as usize != row.len() {
                                    read.add(b"a line of other than its count of samples");
                                }
                                for value in row {
                                    read.add(&value.to_bits().to_le_bytes());
                                }
                            }
                        }
                    }
                    ExrEnd::Read
                }
            };
            let flags = exr_add_outcome(&mut hash, end, &read, theirs);
            past_end += u32::from(flags == 1);
            stopped += u32::from(flags == 2);
        }

        if std::env::var_os("SHOW_EACH").is_some() {
            eprintln!("file {i} {:016x}", hash.value);
        }
        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{name} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{name} bytes of files: {file_total}; read where it depends on what is outside the file: {past_end}, where tinyexr stops: {stopped}, left: {left}\n")
}

/// The `libdeflate` dump: made-up data compressed into zlib streams at a level of
/// libdeflate's, and the streams (and damaged copies, and made-up bytes after a zlib header)
/// decompressed, as `compare/dump_libdeflate.c` does with libdeflate itself.
pub fn libdeflate(seed: u64, count: u32) -> String {
    use rustbcx::libdeflate::{compress_buffer, uncompress_buffer, Error};
    const LEVELS: [i32; 10] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 4];
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let mut total = 0u64;
    let mut out = String::new();
    let add_number = |hash: &mut Hash, value: u32| hash.add(&value.to_le_bytes());
    fn add_uncompress(hash: &mut Hash, input: &[u8], room: usize) {
        let mut out = vec![0u8; room];
        match uncompress_buffer(input, &mut out) {
            Ok(made) => {
                hash.add(&0u32.to_le_bytes());
                hash.add(&(made as u32).to_le_bytes());
                hash.add(&out[..made]);
            }
            Err(Error::Corrupt) => hash.add(&1u32.to_le_bytes()),
            Err(Error::OutOfSpace) => hash.add(&2u32.to_le_bytes()),
        }
    }
    for round in 0..count {
        let mut data = make_data(&mut r);
        if r.next().is_multiple_of(8) {
            let more = 1 + r.next() % 2;
            for _ in 0..more {
                data.extend(make_data(&mut r));
            }
        }
        let level = LEVELS[(r.next() % 10) as usize];
        let mut stream = compress_buffer(level, &data);
        let size = stream.len();
        total += data.len() as u64;
        add_number(&mut hash, data.len() as u32);
        add_number(&mut hash, level as u32);
        add_number(&mut hash, size as u32);
        hash.add(&stream);

        let (how, pick, pick2) = (r.next() % 8, r.next(), r.next());
        let len = data.len();
        let mut room = len;
        let mut n = size;
        if how == 1 && len > 0 {
            room = len - 1 - (pick as usize) % len.min(16);
        } else if how == 2 {
            room = len + (pick % 100) as usize;
        } else if how == 3 {
            let flips = 1 + pick2 % 3;
            for _ in 0..flips {
                let bit = r.next() % (size as u32 * 8);
                stream[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
        } else if how == 4 {
            n = (pick as usize) % (size + 1);
        } else if how == 5 {
            let extra = 1 + pick2 % 9;
            for _ in 0..extra {
                stream.push(r.next() as u8);
            }
            n = size + extra as usize;
        } else if how == 6 {
            stream[(pick as usize) % size] = pick2 as u8;
        }
        add_number(&mut hash, how);
        add_uncompress(&mut hash, &stream[..n], room);

        if round % 4 == 0 {
            let n = 6 + (r.next() % 400) as usize;
            let mut bytes = vec![0x78u8, 0x5e];
            for _ in 2..n {
                bytes.push((r.next() >> 7) as u8);
            }
            if r.next() % 2 != 0 {
                bytes[2] = (bytes[2] & !7) | 1 | (((r.next() % 3) as u8) << 1);
            }
            let room = (r.next() % 3000) as usize;
            add_uncompress(&mut hash, &bytes, room);
        }

        if (round + 1) % 200 == 0 || round + 1 == count {
            out += &format!("libdeflate {} {:016x}\n", round + 1, hash.value);
        }
    }
    out + &format!("libdeflate bytes: {total}\n")
}

/// The `dwa_write` and `dwa_read` dumps of `compare/dump_dwa.c`: made-up blocks of an image
/// compressed as DWAA or DWAB and back (`dwa_write`), or compressed, damaged and decompressed
/// (`dwa_read`).
pub fn dwa(what: &str, seed: u64, count: u32) -> String {
    use rustbcx::dwa::{compress, uncompress, Channel};
    const NAMES: [&str; 24] = ["R", "G", "B", "A", "Y", "BY", "RY", "Z", "r", "g", "b", "a", "red", "green", "blue", "diffuse.R", "diffuse.G", "diffuse.B", "diffuse.A", "spec.R", "spec.B", "left.Y", "mask", "N.x"];
    let read = what == "dwa_read";
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let mut out = String::new();
    let (mut raw_total, mut block_total, mut stored) = (0u64, 0u64, 0u64);
    let (mut decompressed, mut refused) = (0u64, 0u64);

    // The generator's own half of a float: to nearest, ties to even.
    fn to_half(f: f32) -> u16 {
        let bits = f.to_bits();
        let sign = (bits >> 16) & 0x8000;
        let ui = bits & 0x7fff_ffff;
        if ui > 0x7f80_0000 {
            return (sign | 0x7e00 | ((ui >> 13) & 0x3ff)) as u16;
        }
        if ui == 0x7f80_0000 {
            return (sign | 0x7c00) as u16;
        }
        let exp = (ui >> 23) as i32;
        let mant = ui & 0x7f_ffff;
        if exp >= 113 {
            let mut h = (((exp - 112) as u32) << 10) | (mant >> 13);
            let rest = mant & 0x1fff;
            if rest > 0x1000 || (rest == 0x1000 && h & 1 != 0) {
                h += 1;
            }
            return (sign | h.min(0x7c00)) as u16;
        }
        if exp < 102 {
            return sign as u16;
        }
        let m = mant | 0x80_0000;
        let shift = (126 - exp) as u32;
        let mut h = m >> shift;
        let rest = m & ((1 << shift) - 1);
        let halfway = 1u32 << (shift - 1);
        if rest > halfway || (rest == halfway && h & 1 != 0) {
            h += 1;
        }
        (sign | h) as u16
    }

    for i in 0..count {
        // MakeBlock
        let compression = if r.next() % 2 != 0 { 8 } else { 9 };
        let tiled = r.next() % 4 == 0;
        let width = 1 + match r.next() % 8 {
            0 => r.next() % 8,
            1..=4 => r.next() % 64,
            5 | 6 => r.next() % 160,
            _ => r.next() % 400,
        } as usize;
        let most = if compression == 8 { 32u32 } else { 256 };
        let few = r.next() % 8 < 6;
        let lines = 1 + (r.next() % if few { most.min(40) } else { most }) as usize;
        let level = match r.next() % 8 {
            4 => 0.0f32,
            5 => (r.next() % 300) as f32 + (r.next() % 100) as f32 / 100.0,
            6 => (r.next() % 100000) as f32 / 7.0,
            7 => (r.next() % 2000) as f32 / 10.0,
            _ => 45.0,
        };
        let n = 1 + (r.next() % 6) as usize;
        let mut names: Vec<usize> = Vec::new();
        while names.len() < n {
            let name = (r.next() % 24) as usize;
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort_by(|a, b| NAMES[*a].as_bytes().cmp(NAMES[*b].as_bytes()));
        let mut types = Vec::new();
        let mut linear = Vec::new();
        let mut planes: Vec<Vec<u8>> = Vec::new();
        for _ in 0..n {
            let t = r.next() % 10;
            let pixel_type = if t < 6 { 1 } else if t < 9 { 2 } else { 0 };
            let lin = r.next() % 8 == 0;
            let bpe = if pixel_type == 1 { 2 } else { 4 };
            let kind = r.next() % 8;
            let (a, b, c) = (r.next() % 32, r.next() % 32, r.next() % 1000);
            let mut plane = Vec::with_capacity(width * lines * bpe);
            for y in 0..lines {
                for x in 0..width {
                    if pixel_type == 0 {
                        let u = if kind < 4 { r.next() % 256 } else { r.next() };
                        plane.extend_from_slice(&u.to_le_bytes());
                        continue;
                    }
                    if kind == 4 || kind == 5 {
                        let mut bits = r.next();
                        if pixel_type == 1 || kind == 4 {
                            let mut h = bits as u16;
                            if lin && (h & 0x7c00) == 0x7c00 {
                                h &= 0xfbff;
                            }
                            if pixel_type == 1 {
                                plane.extend_from_slice(&h.to_le_bytes());
                                continue;
                            }
                            let h = u32::from(h);
                            let (sign, e, m) = ((h >> 15) << 31, (h >> 10) & 0x1f, h & 0x3ff);
                            bits = if e == 0x1f {
                                sign | 0x7f80_0000 | (m << 13)
                            } else if e != 0 {
                                sign | ((e + 112) << 23) | (m << 13)
                            } else {
                                (m as f32 / 16777216.0).to_bits() | sign
                            };
                        } else if lin && (bits & 0x7f80_0000) == 0x7f80_0000 {
                            bits = 0;
                        }
                        plane.extend_from_slice(&bits.to_le_bytes());
                        continue;
                    }
                    let f = match kind {
                        0 => ((x as i32 * a as i32 + y as i32 * b as i32 + c as i32) as u32 % 4096) as f32 / 1000.0 - 1.0,
                        1 => (r.next() % 20000) as f32 / 5000.0 - 2.0,
                        2 => a as f32 / 100.0,
                        3 => {
                            if r.next() % 16 == 0 {
                                (r.next() % 1000) as f32 / 50.0
                            } else {
                                0.0
                            }
                        }
                        6 => (((x / 8 + y / 8) as u32 + a) % 3) as f32 * 0.5,
                        _ => {
                            let f = (r.next() % 60000) as f32 + 0.5;
                            if r.next() % 2 != 0 {
                                f
                            } else {
                                -f
                            }
                        }
                    };
                    if pixel_type == 1 {
                        plane.extend_from_slice(&to_half(f).to_le_bytes());
                    } else {
                        plane.extend_from_slice(&f.to_bits().to_le_bytes());
                    }
                }
            }
            types.push(pixel_type);
            linear.push(lin);
            planes.push(plane);
        }
        let channels: Vec<Channel> = (0..n).map(|c| Channel { name: NAMES[names[c]], pixel_type: types[c], p_linear: linear[c] }).collect();

        // RawBlock
        let mut raw = Vec::new();
        for y in 0..lines {
            for c in 0..n {
                let row = width * if types[c] == 1 { 2 } else { 4 };
                raw.extend_from_slice(&planes[c][row * y..row * (y + 1)]);
            }
        }

        let mut block = compress(&raw, &channels, width, lines, level, compression == 8 && tiled);
        raw_total += raw.len() as u64;
        block_total += block.len() as u64;
        stored += u64::from(block.len() == raw.len());
        let mut decode = |hash: &mut Hash, data: &[u8]| {
            let mut lines_out = vec![0u8; raw.len()];
            let code = match uncompress(data, &channels, width, lines, &mut lines_out) {
                Ok(()) => {
                    decompressed += 1;
                    0
                }
                Err(code) => {
                    refused += 1;
                    code
                }
            };
            hash.add(&(code as u32).to_le_bytes());
            hash.add(&lines_out);
        };
        if !read {
            hash.add(&(raw.len() as u32).to_le_bytes());
            hash.add(&raw);
            hash.add(&(block.len() as u32).to_le_bytes());
            hash.add(&block);
            decode(&mut hash, &block);
        } else {
            // Damage
            let size = block.len();
            let room = size + 400;
            block.resize(room, 0);
            let put = |data: &mut [u8], i: usize, value: u64| data[8 * i..8 * i + 8].copy_from_slice(&value.to_le_bytes());
            let get = |data: &[u8], i: usize| u64::from_le_bytes(data[8 * i..8 * i + 8].try_into().unwrap());
            let size = match r.next() % 11 {
                10 => 'rules: {
                    if size < 90 || get(&block, 0) != 2 {
                        break 'rules size;
                    }
                    let old = usize::from(block[88]) | (usize::from(block[89]) << 8);
                    if old < 2 || 88 + old > size {
                        break 'rules size;
                    }
                    let mut rules = vec![0u8, 0];
                    let many = 1 + r.next() % 6;
                    for _ in 0..many {
                        let suffix = NAMES[(r.next() % 15) as usize];
                        rules.extend_from_slice(suffix.as_bytes());
                        rules.push(0);
                        let (csc, scheme, nocase) = (r.next() % 4, r.next() % 3, r.next() % 2);
                        rules.push(((csc << 4) | (scheme << 2) | nocase) as u8);
                        rules.push((r.next() % 3) as u8);
                    }
                    let n = rules.len();
                    rules[0] = n as u8;
                    let rest = size - 88 - old;
                    if 88 + n + rest > room {
                        break 'rules size;
                    }
                    block.copy_within(88 + old..size, 88 + n);
                    block[88..88 + n].copy_from_slice(&rules);
                    88 + n + rest
                }
                1 => {
                    let n = 1 + r.next() % 4;
                    for _ in 0..n {
                        if size == 0 {
                            break;
                        }
                        let at = r.next() as usize % size;
                        block[at] ^= (1 + r.next() % 255) as u8;
                    }
                    size
                }
                2 => {
                    if size > 0 {
                        r.next() as usize % size
                    } else {
                        0
                    }
                }
                3 => {
                    if size >= 88 {
                        let i = (r.next() % 11) as usize;
                        let value = match r.next() % 3 {
                            0 => u64::from(r.next() % 2000),
                            1 => get(&block, i).wrapping_add(u64::from(r.next() % 5)).wrapping_sub(2),
                            _ => {
                                let low = u64::from(r.next());
                                low | (u64::from(r.next()) << 32)
                            }
                        };
                        put(&mut block, i, value);
                    }
                    size
                }
                4 => {
                    if size > 88 {
                        let span = (size - 88).min(40);
                        let at = 88 + r.next() as usize % span;
                        block[at] = r.next() as u8;
                    }
                    size
                }
                5 => {
                    let n = r.next() % 20;
                    let mut size = size;
                    for _ in 0..n {
                        if size >= room {
                            break;
                        }
                        block[size] = r.next() as u8;
                        size += 1;
                    }
                    size
                }
                6 => {
                    let n = (88 + r.next() % 300) as usize;
                    let n = n.min(room);
                    for k in 0..n {
                        block[k] = r.next() as u8;
                    }
                    if n >= 88 {
                        for i in 0..11 {
                            let value = if i == 0 { r.next() % 3 } else { r.next() % 400 };
                            put(&mut block, i, u64::from(value));
                        }
                    }
                    n
                }
                7 => {
                    if size > 120 {
                        let n = 1 + r.next() % 3;
                        for _ in 0..n {
                            let at = 120 + r.next() as usize % (size - 120);
                            block[at] ^= (1 + r.next() % 255) as u8;
                        }
                    }
                    size
                }
                8 => {
                    if size >= 88 {
                        let version = r.next() % 2;
                        put(&mut block, 0, u64::from(version));
                    }
                    size
                }
                9 => {
                    if size >= 88 {
                        let ac = r.next() % 3;
                        put(&mut block, 10, u64::from(ac));
                    }
                    size
                }
                _ => size,
            };
            hash.add(&(size as u32).to_le_bytes());
            decode(&mut hash, &block[..size]);
        }
        if (i + 1) % 200 == 0 || i + 1 == count {
            out += &format!("{what} {} {:016x}\n", i + 1, hash.value);
        }
    }
    out + &format!("{what} bytes: {raw_total} as {block_total}, {stored} blocks stored as they are; decompressed: {decompressed}, refused: {refused}\n")
}
