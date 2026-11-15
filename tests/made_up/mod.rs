//! Made-up blocks, the same from a seed here as in `compare/dump.cpp`, and the dumps the tests
//! run as well as the comparison (`examples/dump.rs`).

// (The arithmetic is written as dump.cpp has it; not every user of this module uses all of it.)
#![allow(clippy::manual_is_multiple_of, clippy::needless_range_loop, dead_code)]

use rustbcx::bc7decomp;
use rustbcx::bc7enc::{Bc7Enc, CompressBlockParams};
use rustbcx::ert::{self, ReduceEntropyParams};
use rustbcx::rgbcx::{self, Bc1ApproxMode, Rgbcx};

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
