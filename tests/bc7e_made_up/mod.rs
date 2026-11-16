//! Made-up blocks and settings, the same from a seed here as in `compare/dump_bc7e.cpp`: what the
//! comparison with bc7e.ispc (`examples/dump.rs`) and the recorded results (`tests/bc7e_golden.rs`)
//! pack.

// (The arithmetic is written as dump_bc7e.cpp has
// it; not every user of this module uses all of it.)
#![allow(clippy::manual_is_multiple_of, clippy::needless_range_loop, dead_code)]

use rustbcx::bc7e::CompressBlockParams;

/// One of the original's seven profiles: the settings for an error measure (true: perceptual).
pub type Profile = fn(bool) -> CompressBlockParams;

/// The seven profiles, fastest first, as the dumps name them.
pub const PROFILES: [(&str, Profile); 7] = [
    ("ultrafast", CompressBlockParams::ultrafast),
    ("veryfast", CompressBlockParams::veryfast),
    ("fast", CompressBlockParams::fast),
    ("basic", CompressBlockParams::basic),
    ("slow", CompressBlockParams::slow),
    ("veryslow", CompressBlockParams::veryslow),
    ("slowest", CompressBlockParams::slowest),
];

/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }
}

/// Sixteen made-up pixels of one of eight kinds, the same in both programs.
pub fn make_pixels(random: &mut Random) -> [[u8; 4]; 16] {
    let mut pixels = [[0u8; 4]; 16];
    let kind = random.next() % 8;
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    match kind {
        0 => {
            // noise
            for pixel in &mut pixels {
                for value in pixel {
                    *value = random.next() as u8;
                }
            }
        }
        1 => {
            // from one colour to another, with a little noise
            let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
            for c in 0..4 {
                a[c] = random.next() as u8;
                b[c] = random.next() as u8;
            }
            let noise = random.next() % 9;
            for pixel in &mut pixels {
                let t = (random.next() % 256) as i32;
                for c in 0..4 {
                    let n = if noise != 0 {
                        (random.next() % (noise + 1)) as i32 - (noise / 2) as i32
                    } else {
                        0
                    };
                    pixel[c] = clamp((a[c] as i32 * (255 - t) + b[c] as i32 * t) / 255 + n);
                }
            }
        }
        2 => {
            // greys
            for pixel in &mut pixels {
                let v = random.next() as u8;
                *pixel = [v, v, v, random.next() as u8];
            }
        }
        3 => {
            // greys one step apart
            let base = random.next() % 255;
            for pixel in &mut pixels {
                let v = (base + random.next() % 2) as u8;
                *pixel = [v, v, v, (base + random.next() % 3) as u8];
            }
        }
        4 | 5 => {
            // one colour; one colour but for a pixel
            let mut a = [0u8; 4];
            for value in &mut a {
                *value = random.next() as u8;
            }
            pixels = [a; 16];
            if kind == 5 {
                let at = (random.next() % 16) as usize;
                for c in 0..4 {
                    pixels[at][c] = (a[c] as u32 + 1 + random.next() % 40) as u8;
                }
            }
        }
        6 => {
            // dark, with a few bright pixels
            for pixel in &mut pixels {
                let bright = random.next() % 4 == 0;
                for value in pixel {
                    *value = if bright {
                        random.next()
                    } else {
                        random.next() % 8
                    } as u8;
                }
            }
        }
        _ => {
            // three colours
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

/// The blocks' pixels: half the blocks opaque.
pub fn make_blocks(random: &mut Random, blocks: u32) -> Vec<[[u8; 4]; 16]> {
    (0..blocks)
        .map(|_| {
            let mut pixels = make_pixels(random);
            if random.next() % 2 == 0 {
                for pixel in &mut pixels {
                    pixel[3] = 255;
                }
            }
            pixels
        })
        .collect()
}

/// Settings made up from the seed, kept to those that leave every block a mode to be packed in.
pub fn make_settings(random: &mut Random) -> CompressBlockParams {
    const PARTITIONS: [u32; 8] = [64, 64, 64, 32, 16, 1, 0, 100];
    const PARTITIONS0: [u32; 4] = [16, 16, 8, 1];
    const PASSES: [u32; 8] = [1, 1, 1, 1, 0, 2, 3, 1];
    const TO_TRY: [u32; 8] = [1, 1, 2, 4, 8, 3, 64, 20];
    const MUL: [u32; 4] = [1, 1, 2, 3];
    let mut p = CompressBlockParams::new(random.next() % 2 != 0);
    p.max_partitions_mode[0] = PARTITIONS0[(random.next() % 4) as usize];
    p.max_partitions_mode[1] = PARTITIONS[(random.next() % 8) as usize];
    p.max_partitions_mode[2] = PARTITIONS[(random.next() % 8) as usize];
    p.max_partitions_mode[3] = PARTITIONS[(random.next() % 8) as usize];
    p.max_partitions_mode[7] = PARTITIONS[(random.next() % 8) as usize];
    let weights = random.next() % 4 == 0;
    for c in 0..4 {
        let weight = 1 + random.next() % 255;
        if weights {
            p.weights[c] = weight;
        }
    }
    p.uber_level = random.next() % 5;
    p.refinement_passes = PASSES[(random.next() % 8) as usize];
    p.mode4_rotation_mask = 1 + random.next() % 15;
    p.mode4_index_mask = 1 + random.next() % 3;
    p.mode5_rotation_mask = 1 + random.next() % 15;
    p.uber1_mask = 1 + random.next() % 7;
    p.pbit_search = random.next() % 2 != 0;
    p.mode6_only = random.next() % 8 == 0;
    p.opaque_settings.max_mode13_partitions_to_try = TO_TRY[(random.next() % 8) as usize];
    p.opaque_settings.max_mode0_partitions_to_try = TO_TRY[(random.next() % 8) as usize];
    p.opaque_settings.max_mode2_partitions_to_try = TO_TRY[(random.next() % 8) as usize];
    for m in 0..7 {
        p.opaque_settings.use_mode[m] = random.next() % 4 != 0;
    }
    p.alpha_settings.max_mode7_partitions_to_try = TO_TRY[(random.next() % 8) as usize];
    for c in 0..4 {
        p.alpha_settings.mode67_error_weight_mul[c] = MUL[(random.next() % 4) as usize];
    }
    p.alpha_settings.use_mode4 = random.next() % 4 != 0;
    p.alpha_settings.use_mode5 = random.next() % 4 != 0;
    p.alpha_settings.use_mode6 = random.next() % 4 != 0;
    p.alpha_settings.use_mode7 = random.next() % 4 != 0;
    p.alpha_settings.use_mode4_rotation = random.next() % 2 != 0;
    p.alpha_settings.use_mode5_rotation = random.next() % 2 != 0;

    let use_mode = p.opaque_settings.use_mode;
    if !(use_mode[0]
        || use_mode[1]
        || use_mode[2]
        || use_mode[3]
        || use_mode[6]
        || (!p.perceptual && (use_mode[4] || use_mode[5])))
    {
        p.opaque_settings.use_mode[6] = true;
    }
    if !(p.alpha_settings.use_mode6
        || p.alpha_settings.use_mode7
        || (p.alpha_settings.use_mode5 && p.mode5_rotation_mask & 1 != 0)
        || (p.alpha_settings.use_mode4 && p.mode4_rotation_mask & 1 != 0))
    {
        p.alpha_settings.use_mode6 = true;
    }
    p
}

/// FNV-1a, 64 bits, of the packed blocks; the blocks counted by mode.
pub fn hash(blocks: &[[u8; 16]], modes: &mut [u32; 9]) -> u64 {
    let mut value = 14695981039346656037u64;
    for block in blocks {
        for &byte in block {
            value = (value ^ u64::from(byte)).wrapping_mul(1099511628211);
        }
        modes[(block[0].trailing_zeros() as usize).min(8)] += 1;
    }
    value
}
