//! BC7 block encoding with all eight modes: the port of `bc7e.ispc` (Copyright (C) 2018-2020
//! Binomial LLC, Apache 2.0; the copy in Richard Geldreich's
//! [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo)), function for function.
//!
//! The ISPC original packs several blocks at once, one in each lane of a vector register; this
//! packs them one after another, with the same arithmetic in the same order. Built without
//! fast-math and without fused multiply-add, the original gives the same bytes on every
//! instruction set, and those are the bytes this gives (`compare/compare.sh` checks it).
//!
//! The tables the original fills at start-up in globals are a [`Bc7e`], made by [`Bc7e::new`];
//! `bc7e_compress_blocks` is its method [`Bc7e::compress_blocks`], and the settings,
//! `bc7e_compress_block_params`, are [`CompressBlockParams`], with the original's seven
//! profiles from [`CompressBlockParams::ultrafast`] to [`CompressBlockParams::slowest`].
//!
//! (The partition and anchor tables are the ones [`crate::bc7decomp`] has: the two originals
//! hold the same tables.)

// The code follows the original's line for line, and these are its shapes.
#![allow(clippy::collapsible_else_if, clippy::collapsible_if, clippy::implicit_saturating_sub)]

use crate::bc7decomp::{ColorRgba, ANCHOR_INDEX_SECOND_SUBSET, ANCHOR_INDEX_THIRD_SUBSET_1, ANCHOR_INDEX_THIRD_SUBSET_2, PARTITION2, PARTITION3};
use crate::fmath;
use crate::parallel;
use std::fmt;

/// The bytes of a BC7 block.
pub const BLOCK_SIZE: usize = 16;
/// The most partitions mode 0 has.
pub const MAX_PARTITIONS0: u32 = 16;
/// The most partitions mode 1 has.
pub const MAX_PARTITIONS1: u32 = 64;
/// The most partitions mode 2 has.
pub const MAX_PARTITIONS2: u32 = 64;
/// The most partitions mode 3 has.
pub const MAX_PARTITIONS3: u32 = 64;
/// The most partitions mode 7 has.
pub const MAX_PARTITIONS7: u32 = 64;
/// The highest uber level.
pub const MAX_UBER_LEVEL: u32 = 4;

const CHECKERBOARD_PARTITION_INDEX_2SUBSET: u32 = 34;

/// The settings for blocks without alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpaqueSettings {
    /// How many of the partitions the estimator ranks best are tried in full for modes 1
    /// and 3.
    pub max_mode13_partitions_to_try: u32,
    /// The same for mode 0.
    pub max_mode0_partitions_to_try: u32,
    /// The same for mode 2.
    pub max_mode2_partitions_to_try: u32,
    /// Which of modes 0 to 6 may be used.
    pub use_mode: [bool; 7],
}

/// The settings for blocks with alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaSettings {
    /// How many of the partitions the estimator ranks best are tried in full for mode 7.
    pub max_mode7_partitions_to_try: u32,
    /// Multiplies the four error weights for modes 6 and 7.
    pub mode67_error_weight_mul: [u32; 4],
    pub use_mode4: bool,
    pub use_mode5: bool,
    pub use_mode6: bool,
    pub use_mode7: bool,
    /// Try mode 4 with a colour channel swapped with alpha.
    pub use_mode4_rotation: bool,
    /// Try mode 5 with a colour channel swapped with alpha.
    pub use_mode5_rotation: bool,
}

/// The settings of [`Bc7e::compress_blocks`]: `bc7e_compress_block_params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressBlockParams {
    /// The most partitions the estimator looks at, for each mode.
    pub max_partitions_mode: [u32; 8],
    /// Relative RGBA or YCbCrA weights.
    pub weights: [u32; 4],
    /// From 0 to [`MAX_UBER_LEVEL`]. The higher, the slower and the better.
    pub uber_level: u32,
    /// How many times the endpoints are refitted to the selectors by least squares.
    pub refinement_passes: u32,
    /// Which of mode 4's four rotations may be tried, a bit for each (must not be 0).
    pub mode4_rotation_mask: u32,
    /// Which of mode 4's two index arrangements may be tried, a bit for each (must not be 0).
    pub mode4_index_mask: u32,
    /// Which of mode 5's four rotations may be tried, a bit for each (must not be 0).
    pub mode5_rotation_mask: u32,
    /// Which of uber level 1's three selector variations are tried, a bit for each (must not
    /// be 0).
    pub uber1_mask: u32,
    /// If true, colorspace error is computed in YCbCr space, otherwise RGB.
    pub perceptual: bool,
    /// Try both p-bits of an endpoint, not only the nearer.
    pub pbit_search: bool,
    /// Pack opaque blocks with mode 6 only.
    pub mode6_only: bool,
    pub opaque_settings: OpaqueSettings,
    pub alpha_settings: AlphaSettings,
}

impl CompressBlockParams {
    /// The base settings every profile starts from: `bc7e_compress_block_params_init`.
    pub fn new(perceptual: bool) -> Self {
        CompressBlockParams {
            max_partitions_mode: [MAX_PARTITIONS0, MAX_PARTITIONS1, MAX_PARTITIONS2, MAX_PARTITIONS3, 0, 0, 0, MAX_PARTITIONS7],
            weights: if perceptual { [128, 64, 16, 256] } else { [1, 1, 1, 1] },
            uber_level: 0,
            refinement_passes: 1,
            mode4_rotation_mask: 0xF,
            mode4_index_mask: 3,
            mode5_rotation_mask: 0xF,
            uber1_mask: 7,
            perceptual,
            pbit_search: false,
            mode6_only: false,
            opaque_settings: OpaqueSettings { max_mode13_partitions_to_try: 1, max_mode0_partitions_to_try: 1, max_mode2_partitions_to_try: 1, use_mode: [true; 7] },
            alpha_settings: AlphaSettings {
                max_mode7_partitions_to_try: 1,
                mode67_error_weight_mul: [1, 1, 1, 1],
                use_mode4: true,
                use_mode5: true,
                use_mode6: true,
                use_mode7: true,
                use_mode4_rotation: true,
                use_mode5_rotation: true,
            },
        }
    }

    /// `bc7e_compress_block_params_init_slowest`
    pub fn slowest(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        p.opaque_settings.max_mode13_partitions_to_try = 4;
        p.opaque_settings.max_mode0_partitions_to_try = 4;
        p.opaque_settings.max_mode2_partitions_to_try = 4;
        p.alpha_settings.max_mode7_partitions_to_try = 4;
        p.pbit_search = true;
        p.uber_level = 4;
        p
    }

    /// `bc7e_compress_block_params_init_veryslow`
    pub fn veryslow(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        p.opaque_settings.max_mode13_partitions_to_try = 2;
        p.opaque_settings.max_mode0_partitions_to_try = 2;
        p.opaque_settings.max_mode2_partitions_to_try = 2;
        p.alpha_settings.max_mode7_partitions_to_try = 2;
        p.pbit_search = true;
        p.uber_level = 2;
        p
    }

    /// `bc7e_compress_block_params_init_slow`
    pub fn slow(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        p.alpha_settings.max_mode7_partitions_to_try = 2;
        p.pbit_search = true;
        p.uber_level = 0;
        p
    }

    /// `bc7e_compress_block_params_init_basic`
    pub fn basic(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        if perceptual {
            p.opaque_settings.use_mode[0] = false;
            p.opaque_settings.use_mode[2] = false;
            p.opaque_settings.use_mode[3] = false;
            p.opaque_settings.use_mode[4] = false;
            p.opaque_settings.use_mode[5] = false;
            p.pbit_search = false;
            p.uber_level = 1;
        } else {
            p.max_partitions_mode[1] = 32;
            p.max_partitions_mode[2] = 32;
            p.max_partitions_mode[3] = 32;
            p.max_partitions_mode[7] = 32;
            p.opaque_settings.use_mode[2] = false;
            p.pbit_search = false;
            p.uber_level = 1;
        }
        p
    }

    /// `bc7e_compress_block_params_init_fast`
    pub fn fast(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        if perceptual {
            p.opaque_settings.use_mode[0] = false;
            p.opaque_settings.use_mode[2] = false;
            p.opaque_settings.use_mode[3] = false;
            p.opaque_settings.use_mode[4] = false;
            p.opaque_settings.use_mode[5] = false;
            p.alpha_settings.use_mode5 = false;
            p.opaque_settings.max_mode13_partitions_to_try = 1;
            p.pbit_search = false;
            p.uber_level = 0;
        } else {
            p.opaque_settings.use_mode[0] = false;
            p.opaque_settings.use_mode[2] = false;
            p.opaque_settings.use_mode[4] = false;
            p.opaque_settings.use_mode[5] = false;
            p.alpha_settings.use_mode5 = false;
            p.opaque_settings.max_mode13_partitions_to_try = 2;
            p.pbit_search = false;
            p.uber_level = 0;
        }
        p
    }

    /// `bc7e_compress_block_params_init_veryfast`
    pub fn veryfast(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        if perceptual {
            p.opaque_settings.use_mode[0] = false;
            p.opaque_settings.use_mode[2] = false;
            p.opaque_settings.use_mode[3] = false;
            p.opaque_settings.use_mode[4] = false;
            p.opaque_settings.use_mode[5] = false;
            p.alpha_settings.use_mode5 = false;
            p.pbit_search = false;
            p.uber_level = 0;
        } else {
            p.opaque_settings.use_mode[2] = false;
            p.opaque_settings.use_mode[4] = false;
            p.opaque_settings.use_mode[5] = false;
            p.alpha_settings.use_mode5 = false;
            p.pbit_search = false;
            p.uber_level = 0;
        }
        p
    }

    /// `bc7e_compress_block_params_init_ultrafast`
    pub fn ultrafast(perceptual: bool) -> Self {
        let mut p = Self::new(perceptual);
        p.mode6_only = true;
        p.alpha_settings.use_mode4 = true;
        p.alpha_settings.use_mode5 = true;
        p.alpha_settings.use_mode7 = false;
        p.mode4_rotation_mask = 1 + 4;
        p.mode4_index_mask = 3;
        p.mode5_rotation_mask = 1;
        p.pbit_search = false;
        p.uber_level = 0;
        p
    }
}

// Helpers. (ISPC's `min`, `max` and `clamp` on floats are the processor's: the second operand
// when either is not a number.)

/// A float to an int as the processor's instruction does it: toward zero, and the lowest int
/// for what doesn't fit or is not a number.
#[inline]
fn f2i(value: f32) -> i32 {
    if value >= -2_147_483_648.0 && value < 2_147_483_648.0 {
        value as i32
    } else {
        i32::MIN
    }
}

#[inline]
fn minimumf(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn maximumf(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

#[inline]
fn clampf(value: f32, low: f32, high: f32) -> f32 {
    minimumf(maximumf(value, low), high)
}

#[inline]
fn clampi(value: i32, low: i32, high: i32) -> i32 {
    value.max(low).min(high)
}

#[inline]
fn saturate(value: f32) -> f32 {
    clampf(value, 0.0, 1.0)
}

#[inline]
fn iabs32(v: i32) -> i32 {
    let msk = (v >> 31) as u32;
    ((v as u32) ^ msk).wrapping_sub(msk) as i32
}

#[inline]
fn squaref(s: f32) -> f32 {
    s * s
}

/// A colour with room to spare in each channel: `color_quad_i`.
type ColorQuadI = [i32; 4];

type Vec4F = [f32; 4];

#[inline]
fn color_quad_i_set_clamped(r: i32, g: i32, b: i32, a: i32) -> ColorQuadI {
    [clampi(r, 0, 255), clampi(g, 0, 255), clampi(b, 0, 255), clampi(a, 0, 255)]
}

#[inline]
fn vec4f_saturate(v: &Vec4F) -> Vec4F {
    [saturate(v[0]), saturate(v[1]), saturate(v[2]), saturate(v[3])]
}

#[inline]
fn vec4f_from_color(c: &ColorQuadI) -> Vec4F {
    [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]
}

#[inline]
fn vec4f_add(lhs: &Vec4F, rhs: &Vec4F) -> Vec4F {
    [lhs[0] + rhs[0], lhs[1] + rhs[1], lhs[2] + rhs[2], lhs[3] + rhs[3]]
}

#[inline]
fn vec4f_sub(lhs: &Vec4F, rhs: &Vec4F) -> Vec4F {
    [lhs[0] - rhs[0], lhs[1] - rhs[1], lhs[2] - rhs[2], lhs[3] - rhs[3]]
}

#[inline]
fn vec4f_dot(lhs: &Vec4F, rhs: &Vec4F) -> f32 {
    lhs[0] * rhs[0] + lhs[1] * rhs[1] + lhs[2] * rhs[2] + lhs[3] * rhs[3]
}

#[inline]
fn vec4f_mul(lhs: &Vec4F, s: f32) -> Vec4F {
    [lhs[0] * s, lhs[1] * s, lhs[2] * s, lhs[3] * s]
}

#[inline]
fn vec4f_normalize_in_place(v: &mut Vec4F) {
    let mut s = v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3];

    if s != 0.0 {
        s = 1.0 / s.sqrt();

        v[0] *= s;
        v[1] *= s;
        v[2] *= s;
        v[3] *= s;
    }
}

// Various BC7 tables
const WEIGHTS2: [u32; 4] = [0, 21, 43, 64];
const WEIGHTS3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

// Precomputed weight constants used during least fit determination. For each entry in the
// weights: w * w, (1.0f - w) * w, (1.0f - w) * (1.0f - w), w
#[rustfmt::skip]
const WEIGHTS2X: [f32; 4 * 4] = [0.000000, 0.000000, 1.000000, 0.000000, 0.107666, 0.220459, 0.451416, 0.328125, 0.451416, 0.220459, 0.107666, 0.671875, 1.000000, 0.000000, 0.000000, 1.000000];
#[rustfmt::skip]
const WEIGHTS3X: [f32; 8 * 4] = [0.000000, 0.000000, 1.000000, 0.000000, 0.019775, 0.120850, 0.738525, 0.140625, 0.079102, 0.202148, 0.516602, 0.281250, 0.177979, 0.243896, 0.334229, 0.421875, 0.334229, 0.243896, 0.177979, 0.578125, 0.516602, 0.202148,
    0.079102, 0.718750, 0.738525, 0.120850, 0.019775, 0.859375, 1.000000, 0.000000, 0.000000, 1.000000];
#[rustfmt::skip]
const WEIGHTS4X: [f32; 16 * 4] = [0.000000, 0.000000, 1.000000, 0.000000, 0.003906, 0.058594, 0.878906, 0.062500, 0.019775, 0.120850, 0.738525, 0.140625, 0.041260, 0.161865, 0.635010, 0.203125, 0.070557, 0.195068, 0.539307, 0.265625, 0.107666, 0.220459,
    0.451416, 0.328125, 0.165039, 0.241211, 0.352539, 0.406250, 0.219727, 0.249023, 0.282227, 0.468750, 0.282227, 0.249023, 0.219727, 0.531250, 0.352539, 0.241211, 0.165039, 0.593750, 0.451416, 0.220459, 0.107666, 0.671875, 0.539307, 0.195068, 0.070557, 0.734375,
    0.635010, 0.161865, 0.041260, 0.796875, 0.738525, 0.120850, 0.019775, 0.859375, 0.878906, 0.058594, 0.003906, 0.937500, 1.000000, 0.000000, 0.000000, 1.000000];

const PARTITION1: [u8; 16] = [0; 16];

const NUM_SUBSETS: [u32; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
const PARTITION_BITS: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
const COLOR_INDEX_BITCOUNT: [u32; 8] = [3, 3, 2, 2, 2, 2, 4, 2];

fn get_bc7_color_index_size(mode: u32, index_selection_bit: u32) -> u32 {
    COLOR_INDEX_BITCOUNT[mode as usize] + index_selection_bit
}

const ALPHA_INDEX_BITCOUNT: [u32; 8] = [0, 0, 0, 0, 3, 2, 4, 2];

fn get_bc7_alpha_index_size(mode: u32, index_selection_bit: u32) -> u32 {
    ALPHA_INDEX_BITCOUNT[mode as usize].wrapping_sub(index_selection_bit)
}

const MODE_HAS_P_BITS: [u32; 8] = [1, 1, 0, 1, 0, 0, 1, 1];
const MODE_HAS_SHARED_P_BITS: [u32; 8] = [0, 1, 0, 0, 0, 0, 0, 0];
const COLOR_PRECISION_TABLE: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
const ALPHA_PRECISION_TABLE: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];

fn get_bc7_mode_has_seperate_alpha_selectors(mode: u32) -> bool {
    mode == 4 || mode == 5
}

#[derive(Clone, Copy, Default)]
struct EndpointErr {
    error: u32,
    lo: u8,
    hi: u8,
}

const MODE_1_OPTIMAL_INDEX: u32 = 2;
const MODE_7_OPTIMAL_INDEX: u32 = 1;
const MODE_6_OPTIMAL_INDEX: u32 = 5;
const MODE_4_OPTIMAL_INDEX3: u32 = 2;
const MODE_4_OPTIMAL_INDEX2: u32 = 1;
const MODE_5_OPTIMAL_INDEX: u32 = 1;
const MODE_0_OPTIMAL_INDEX: u32 = 2;

/// The BC7 encoder with the tables it makes at start-up.
#[derive(Clone)]
pub struct Bc7e {
    /// By component value and p-bit.
    mode_1_optimal_endpoints: [[EndpointErr; 2]; 256],
    /// By component value, the high endpoint's p-bit and the low endpoint's.
    mode_7_optimal_endpoints: [[[EndpointErr; 2]; 2]; 256],
    /// By component value, the high endpoint's p-bit and the low endpoint's.
    mode_6_optimal_endpoints: Box<[[[EndpointErr; 2]; 2]; 256]>,
    /// By component value: the low endpoint, and the high one eight bits up. For 3-bit
    /// selectors.
    mode_4_optimal_endpoints3: [u32; 256],
    /// The same for 2-bit selectors.
    mode_4_optimal_endpoints2: [u32; 256],
    mode_5_optimal_endpoints: [u32; 256],
    /// By component value, the high endpoint's p-bit and the low endpoint's.
    mode_0_optimal_endpoints: [[[EndpointErr; 2]; 2]; 256],
}

impl fmt::Debug for Bc7e {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bc7e").finish_non_exhaustive()
    }
}

impl Default for Bc7e {
    fn default() -> Self {
        Self::new()
    }
}

/// The endpoints, of `count` steps each, whose colour at `weight` of the way from the low to
/// the high is nearest `c`; `low` and `high` turn a step into its 8-bit value. An earlier find
/// is kept unless this one is better: mode 7's table is built that way.
fn search_endpoints(best: &mut EndpointErr, c: i32, count: u32, weight: u32, low: impl Fn(u32) -> u32, high: impl Fn(u32) -> u32) {
    for l in 0..count {
        let low = low(l);

        for h in 0..count {
            let high = high(h);

            let k = ((low * (64 - weight) + high * weight + 32) >> 6) as i32;

            let err = (k - c) * (k - c);
            if (err as u32) < best.error {
                best.error = err as u32;
                best.lo = l as u8;
                best.hi = h as u8;
            }
        }
    }
}

impl Bc7e {
    /// The encoder with its tables: `bc7e_compress_block_init`. The tables are made the first
    /// time and copied from then on.
    pub fn new() -> Self {
        static TABLES: std::sync::OnceLock<Bc7e> = std::sync::OnceLock::new();
        TABLES.get_or_init(Bc7e::make_tables).clone()
    }

    /// Makes the tables.
    fn make_tables() -> Self {
        let empty = EndpointErr::default();
        let mut tables = Bc7e {
            mode_1_optimal_endpoints: [[empty; 2]; 256],
            mode_7_optimal_endpoints: [[[empty; 2]; 2]; 256],
            mode_6_optimal_endpoints: Box::new([[[empty; 2]; 2]; 256]),
            mode_4_optimal_endpoints3: [0; 256],
            mode_4_optimal_endpoints2: [0; 256],
            mode_5_optimal_endpoints: [0; 256],
            mode_0_optimal_endpoints: [[[empty; 2]; 2]; 256],
        };
        let fresh = EndpointErr { error: u16::MAX as u32, lo: 0, hi: 0 };

        // Mode 0: 444.1
        for c in 0..256 {
            for hp in 0..2u32 {
                for lp in 0..2u32 {
                    let mut best = fresh;
                    let expand = |v: u32, p: u32| {
                        let x = ((v << 1) | p) << 3;
                        x | (x >> 5)
                    };
                    search_endpoints(&mut best, c as i32, 16, WEIGHTS3[MODE_0_OPTIMAL_INDEX as usize], |l| expand(l, lp), |h| expand(h, hp));
                    tables.mode_0_optimal_endpoints[c][hp as usize][lp as usize] = best;
                }
            }
        }

        // Mode 1: 666.1
        for c in 0..256 {
            for lp in 0..2u32 {
                let mut best = fresh;
                let expand = |v: u32| {
                    let x = ((v << 1) | lp) << 1;
                    x | (x >> 7)
                };
                search_endpoints(&mut best, c as i32, 64, WEIGHTS3[MODE_1_OPTIMAL_INDEX as usize], expand, expand);
                tables.mode_1_optimal_endpoints[c][lp as usize] = best;
            }
        }

        // Mode 6: 777.1 4-bit indices
        for c in 0..256 {
            for hp in 0..2u32 {
                for lp in 0..2u32 {
                    let mut best = fresh;
                    search_endpoints(&mut best, c as i32, 128, WEIGHTS4[MODE_6_OPTIMAL_INDEX as usize], |l| (l << 1) | lp, |h| (h << 1) | hp);
                    tables.mode_6_optimal_endpoints[c][hp as usize][lp as usize] = best;
                }
            }
        }

        // Mode 5: 777 2-bit indices
        for c in 0..256 {
            let mut best = fresh;
            let expand = |v: u32| {
                let x = v << 1;
                x | (x >> 7)
            };
            search_endpoints(&mut best, c as i32, 128, WEIGHTS2[MODE_5_OPTIMAL_INDEX as usize], expand, expand);
            tables.mode_5_optimal_endpoints[c] = best.lo as u32 | ((best.hi as u32) << 8);
        }

        // Mode 4: 555 3-bit indices, then 2-bit indices
        for c in 0..256 {
            let expand = |v: u32| {
                let x = v << 3;
                x | (x >> 5)
            };
            let mut best = fresh;
            search_endpoints(&mut best, c as i32, 32, WEIGHTS3[MODE_4_OPTIMAL_INDEX3 as usize], expand, expand);
            tables.mode_4_optimal_endpoints3[c] = best.lo as u32 | ((best.hi as u32) << 8);

            let mut best = fresh;
            search_endpoints(&mut best, c as i32, 32, WEIGHTS2[MODE_4_OPTIMAL_INDEX2 as usize], expand, expand);
            tables.mode_4_optimal_endpoints2[c] = best.lo as u32 | ((best.hi as u32) << 8);
        }

        // Mode 7: 555.1 2-bit indices
        for c in 0..256 {
            // (One best for all four pairs of p-bits, as the original has it: a later pair
            // holds an earlier pair's endpoints unless its own are better.)
            let mut best = fresh;
            for hp in 0..2u32 {
                for lp in 0..2u32 {
                    let expand = |v: u32, p: u32| {
                        let x = ((v << 1) | p) << 2;
                        x | (x >> 6)
                    };
                    search_endpoints(&mut best, c as i32, 32, WEIGHTS2[MODE_7_OPTIMAL_INDEX as usize], |l| expand(l, lp), |h| expand(h, hp));
                    tables.mode_7_optimal_endpoints[c][hp as usize][lp as usize] = best;
                }
            }
        }

        tables
    }
}

fn compute_least_squares_endpoints_rgba(n: usize, selectors: &[i32], selector_weights: &[f32], xl: &mut Vec4F, xh: &mut Vec4F, colors: &[ColorQuadI]) {
    // Least squares using normal equations: http://www.cs.cornell.edu/~bindel/class/cs3220-s12/notes/lec10.pdf
    // I did this in matrix form first, expanded out all the ops, then optimized it a bit.
    let (mut z00, mut z10, mut z11) = (0.0f32, 0.0f32, 0.0f32);
    let (mut q00_r, mut t_r) = (0.0f32, 0.0f32);
    let (mut q00_g, mut t_g) = (0.0f32, 0.0f32);
    let (mut q00_b, mut t_b) = (0.0f32, 0.0f32);
    let (mut q00_a, mut t_a) = (0.0f32, 0.0f32);
    for i in 0..n {
        let sel = selectors[i] as u32 as usize;
        z00 += selector_weights[sel * 4];
        z10 += selector_weights[sel * 4 + 1];
        z11 += selector_weights[sel * 4 + 2];
        let w = selector_weights[sel * 4 + 3];
        q00_r += w * colors[i][0] as f32;
        t_r += colors[i][0] as f32;
        q00_g += w * colors[i][1] as f32;
        t_g += colors[i][1] as f32;
        q00_b += w * colors[i][2] as f32;
        t_b += colors[i][2] as f32;
        q00_a += w * colors[i][3] as f32;
        t_a += colors[i][3] as f32;
    }

    let q10_r = t_r - q00_r;
    let q10_g = t_g - q00_g;
    let q10_b = t_b - q00_b;
    let q10_a = t_a - q00_a;

    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det != 0.0 {
        det = 1.0 / det;
    }

    let iz00 = z11 * det;
    let iz01 = -z01 * det;
    let iz10 = -z10 * det;
    let iz11 = z00 * det;

    xl[0] = iz00 * q00_r + iz01 * q10_r;
    xh[0] = iz10 * q00_r + iz11 * q10_r;
    xl[1] = iz00 * q00_g + iz01 * q10_g;
    xh[1] = iz10 * q00_g + iz11 * q10_g;
    xl[2] = iz00 * q00_b + iz01 * q10_b;
    xh[2] = iz10 * q00_b + iz11 * q10_b;
    xl[3] = iz00 * q00_a + iz01 * q10_a;
    xh[3] = iz10 * q00_a + iz11 * q10_a;
}

/// (Leaves the fourth of `xl` and `xh` as they were.)
fn compute_least_squares_endpoints_rgb(n: usize, selectors: &[i32], selector_weights: &[f32], xl: &mut Vec4F, xh: &mut Vec4F, colors: &[ColorQuadI]) {
    let (mut z00, mut z10, mut z11) = (0.0f32, 0.0f32, 0.0f32);
    let (mut q00_r, mut t_r) = (0.0f32, 0.0f32);
    let (mut q00_g, mut t_g) = (0.0f32, 0.0f32);
    let (mut q00_b, mut t_b) = (0.0f32, 0.0f32);
    for i in 0..n {
        let sel = selectors[i] as u32 as usize;
        z00 += selector_weights[sel * 4];
        z10 += selector_weights[sel * 4 + 1];
        z11 += selector_weights[sel * 4 + 2];
        let w = selector_weights[sel * 4 + 3];
        q00_r += w * colors[i][0] as f32;
        t_r += colors[i][0] as f32;
        q00_g += w * colors[i][1] as f32;
        t_g += colors[i][1] as f32;
        q00_b += w * colors[i][2] as f32;
        t_b += colors[i][2] as f32;
    }

    let q10_r = t_r - q00_r;
    let q10_g = t_g - q00_g;
    let q10_b = t_b - q00_b;

    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det != 0.0 {
        det = 1.0 / det;
    }

    let iz00 = z11 * det;
    let iz01 = -z01 * det;
    let iz10 = -z10 * det;
    let iz11 = z00 * det;

    xl[0] = iz00 * q00_r + iz01 * q10_r;
    xh[0] = iz10 * q00_r + iz11 * q10_r;
    xl[1] = iz00 * q00_g + iz01 * q10_g;
    xh[1] = iz10 * q00_g + iz11 * q10_g;
    xl[2] = iz00 * q00_b + iz01 * q10_b;
    xh[2] = iz10 * q00_b + iz11 * q10_b;
}

/// The low and the high alpha.
fn compute_least_squares_endpoints_a(n: usize, selectors: &[i32], selector_weights: &[f32], colors: &[ColorQuadI]) -> (f32, f32) {
    // Least squares using normal equations: http://www.cs.cornell.edu/~bindel/class/cs3220-s12/notes/lec10.pdf
    // I did this in matrix form first, expanded out all the ops, then optimized it a bit.
    let (mut z00, mut z10, mut z11) = (0.0f32, 0.0f32, 0.0f32);
    let (mut q00_a, mut t_a) = (0.0f32, 0.0f32);
    for i in 0..n {
        let sel = selectors[i] as u32 as usize;
        z00 += selector_weights[sel * 4];
        z10 += selector_weights[sel * 4 + 1];
        z11 += selector_weights[sel * 4 + 2];
        let w = selector_weights[sel * 4 + 3];
        q00_a += w * colors[i][3] as f32;
        t_a += colors[i][3] as f32;
    }

    let q10_a = t_a - q00_a;

    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det != 0.0 {
        det = 1.0 / det;
    }

    let iz00 = z11 * det;
    let iz01 = -z01 * det;
    let iz10 = -z10 * det;
    let iz11 = z00 * det;

    (iz00 * q00_a + iz01 * q10_a, iz10 * q00_a + iz11 * q10_a)
}

/// What a subset is packed with: `color_cell_compressor_params`.
#[derive(Clone, Copy)]
struct ColorCellCompressorParams {
    num_selector_weights: u32,
    selector_weights: &'static [u32],
    selector_weightsx: &'static [f32],
    comp_bits: u32,
    weights: [u32; 4],
    has_alpha: bool,
    has_pbits: bool,
    endpoints_share_pbit: bool,
    perceptual: bool,
}

impl ColorCellCompressorParams {
    /// `color_cell_compressor_params_clear`
    fn new() -> Self {
        ColorCellCompressorParams {
            num_selector_weights: 0,
            selector_weights: &[],
            selector_weightsx: &[],
            comp_bits: 0,
            weights: [1, 1, 1, 1],
            has_alpha: false,
            has_pbits: false,
            endpoints_share_pbit: false,
            perceptual: false,
        }
    }
}

/// The best way found to pack a subset: `color_cell_compressor_results`. (The original
/// points at its selectors and at a scratch copy of them; here it holds both.)
#[derive(Clone, Copy)]
struct ColorCellCompressorResults {
    best_overall_err: u64,
    low_endpoint: ColorQuadI,
    high_endpoint: ColorQuadI,
    pbits: [u32; 2],
    selectors: [i32; 16],
    selectors_temp: [i32; 16],
}

impl ColorCellCompressorResults {
    fn new() -> Self {
        ColorCellCompressorResults { best_overall_err: 0, low_endpoint: [0; 4], high_endpoint: [0; 4], pbits: [0; 2], selectors: [0; 16], selectors_temp: [0; 16] }
    }
}

#[inline]
fn scale_color(c: &ColorQuadI, params: &ColorCellCompressorParams) -> ColorQuadI {
    let mut results = [0i32; 4];

    let n = params.comp_bits + u32::from(params.has_pbits);
    debug_assert!((4..=8).contains(&n));

    for i in 0..4 {
        let mut v = (c[i] as u32) << (8 - n);
        v |= v >> n;
        debug_assert!(v <= 255);
        results[i] = v as i32;
    }

    results
}

const PR_WEIGHT: f32 = (0.5f32 / (1.0f32 - 0.2126f32)) * (0.5f32 / (1.0f32 - 0.2126f32));
const PB_WEIGHT: f32 = (0.5f32 / (1.0f32 - 0.0722f32)) * (0.5f32 / (1.0f32 - 0.0722f32));

#[inline]
fn compute_color_distance_rgb(e1: &ColorQuadI, e2: &ColorQuadI, perceptual: bool, weights: &[u32; 4]) -> u64 {
    if perceptual {
        let l1 = e1[0] as f32 * 0.2126f32 + e1[1] as f32 * 0.7152f32 + e1[2] as f32 * 0.0722f32;
        let cr1 = e1[0] as f32 - l1;
        let cb1 = e1[2] as f32 - l1;

        let l2 = e2[0] as f32 * 0.2126f32 + e2[1] as f32 * 0.7152f32 + e2[2] as f32 * 0.0722f32;
        let cr2 = e2[0] as f32 - l2;
        let cb2 = e2[2] as f32 - l2;

        let dl = l1 - l2;
        let dcr = cr1 - cr2;
        let dcb = cb1 - cb2;

        (weights[0] as f32 * (dl * dl) + weights[1] as f32 * PR_WEIGHT * (dcr * dcr) + weights[2] as f32 * PB_WEIGHT * (dcb * dcb)) as i64 as u64
    } else {
        let dr = e1[0] as f32 - e2[0] as f32;
        let dg = e1[1] as f32 - e2[1] as f32;
        let db = e1[2] as f32 - e2[2] as f32;

        (weights[0] as f32 * dr * dr + weights[1] as f32 * dg * dg + weights[2] as f32 * db * db) as i64 as u64
    }
}

#[inline]
fn compute_color_distance_rgba(e1: &ColorQuadI, e2: &ColorQuadI, perceptual: bool, weights: &[u32; 4]) -> u64 {
    let da = e1[3] as f32 - e2[3] as f32;
    let a_err = weights[3] as f32 * (da * da);

    if perceptual {
        let l1 = e1[0] as f32 * 0.2126f32 + e1[1] as f32 * 0.7152f32 + e1[2] as f32 * 0.0722f32;
        let cr1 = e1[0] as f32 - l1;
        let cb1 = e1[2] as f32 - l1;

        let l2 = e2[0] as f32 * 0.2126f32 + e2[1] as f32 * 0.7152f32 + e2[2] as f32 * 0.0722f32;
        let cr2 = e2[0] as f32 - l2;
        let cb2 = e2[2] as f32 - l2;

        let dl = l1 - l2;
        let dcr = cr1 - cr2;
        let dcb = cb1 - cb2;

        (weights[0] as f32 * (dl * dl) + weights[1] as f32 * PR_WEIGHT * (dcr * dcr) + weights[2] as f32 * PB_WEIGHT * (dcb * dcb) + a_err) as i64 as u64
    } else {
        let dr = e1[0] as f32 - e2[0] as f32;
        let dg = e1[1] as f32 - e2[1] as f32;
        let db = e1[2] as f32 - e2[2] as f32;

        (weights[0] as f32 * dr * dr + weights[1] as f32 * dg * dg + weights[2] as f32 * db * db + a_err) as i64 as u64
    }
}

// The five `pack_mode*_to_one_color`: a subset packed as one colour from the tables. Each
// leaves the endpoints, the p-bits and the error in the results. The original also fills the
// subset's selectors with the mode's optimal index; here the caller does, with the index
// these return.

impl Bc7e {
    fn pack_mode1_to_one_color(&self, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        let table = &self.mode_1_optimal_endpoints;
        let (r, g, b) = (r as usize, g as usize, b as usize);

        let mut best_err = u32::MAX;
        let mut best_p = 0usize;

        for p in 0..2 {
            let err = table[r][p].error + table[g][p].error + table[b][p].error;
            if err < best_err {
                best_err = err;
                best_p = p;
            }
        }

        let (er, eg, eb) = (&table[r][best_p], &table[g][best_p], &table[b][best_p]);

        results.low_endpoint = [er.lo as i32, eg.lo as i32, eb.lo as i32, 0];
        results.high_endpoint = [er.hi as i32, eg.hi as i32, eb.hi as i32, 0];

        results.pbits[0] = best_p as u32;
        results.pbits[1] = 0;

        let mut p = [0i32; 4];

        for i in 0..3 {
            let mut low = (((results.low_endpoint[i] as u32) << 1) | results.pbits[0]) << 1;
            low |= low >> 7;

            let mut high = (((results.high_endpoint[i] as u32) << 1) | results.pbits[0]) << 1;
            high |= high >> 7;

            p[i] = ((low * (64 - WEIGHTS3[MODE_1_OPTIMAL_INDEX as usize]) + high * WEIGHTS3[MODE_1_OPTIMAL_INDEX as usize] + 32) >> 6) as i32;
        }

        p[3] = 255;

        let mut total_err = 0u64;
        for pixel in pixels {
            total_err = total_err.wrapping_add(compute_color_distance_rgb(&p, pixel, params.perceptual, &params.weights));
        }

        results.best_overall_err = total_err;

        (total_err, MODE_1_OPTIMAL_INDEX as i32)
    }

    /// (Leaves the p-bits as they were: these modes have none.)
    fn pack_mode24_to_one_color(&self, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        let (r, g, b) = (r as usize, g as usize, b as usize);
        let wide = params.num_selector_weights == 8;

        let (er, eg, eb) = if wide {
            (self.mode_4_optimal_endpoints3[r], self.mode_4_optimal_endpoints3[g], self.mode_4_optimal_endpoints3[b])
        } else {
            (self.mode_4_optimal_endpoints2[r], self.mode_4_optimal_endpoints2[g], self.mode_4_optimal_endpoints2[b])
        };

        results.low_endpoint = [(er & 0xFF) as i32, (eg & 0xFF) as i32, (eb & 0xFF) as i32, 0];
        results.high_endpoint = [(er >> 8) as i32, (eg >> 8) as i32, (eb >> 8) as i32, 0];

        let mut p = [0i32; 4];

        for i in 0..3 {
            let mut low = (results.low_endpoint[i] as u32) << 3;
            low |= low >> 5;

            let mut high = (results.high_endpoint[i] as u32) << 3;
            high |= high >> 5;

            p[i] = if wide {
                ((low * (64 - WEIGHTS3[MODE_4_OPTIMAL_INDEX3 as usize]) + high * WEIGHTS3[MODE_4_OPTIMAL_INDEX3 as usize] + 32) >> 6) as i32
            } else {
                ((low * (64 - WEIGHTS2[MODE_4_OPTIMAL_INDEX2 as usize]) + high * WEIGHTS2[MODE_4_OPTIMAL_INDEX2 as usize] + 32) >> 6) as i32
            };
        }

        p[3] = 255;

        let mut total_err = 0u64;
        for pixel in pixels {
            total_err = total_err.wrapping_add(compute_color_distance_rgb(&p, pixel, params.perceptual, &params.weights));
        }

        results.best_overall_err = total_err;

        (total_err, if wide { MODE_4_OPTIMAL_INDEX3 } else { MODE_4_OPTIMAL_INDEX2 } as i32)
    }

    fn pack_mode0_to_one_color(&self, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        let table = &self.mode_0_optimal_endpoints;
        let (r, g, b) = (r as usize, g as usize, b as usize);

        let mut best_err = u32::MAX;
        let mut best_p = 0usize;

        for p in 0..4usize {
            let err = table[r][p >> 1][p & 1].error + table[g][p >> 1][p & 1].error + table[b][p >> 1][p & 1].error;
            if err < best_err {
                best_err = err;
                best_p = p;
            }
        }

        let (er, eg, eb) = (&table[r][best_p >> 1][best_p & 1], &table[g][best_p >> 1][best_p & 1], &table[b][best_p >> 1][best_p & 1]);

        results.low_endpoint = [er.lo as i32, eg.lo as i32, eb.lo as i32, 0];
        results.high_endpoint = [er.hi as i32, eg.hi as i32, eb.hi as i32, 0];

        results.pbits[0] = (best_p & 1) as u32;
        results.pbits[1] = (best_p >> 1) as u32;

        let mut p = [0i32; 4];

        for i in 0..3 {
            let mut low = (((results.low_endpoint[i] as u32) << 1) | results.pbits[0]) << 3;
            low |= low >> 5;

            let mut high = (((results.high_endpoint[i] as u32) << 1) | results.pbits[1]) << 3;
            high |= high >> 5;

            p[i] = ((low * (64 - WEIGHTS3[MODE_0_OPTIMAL_INDEX as usize]) + high * WEIGHTS3[MODE_0_OPTIMAL_INDEX as usize] + 32) >> 6) as i32;
        }

        p[3] = 255;

        let mut total_err = 0u64;
        for pixel in pixels {
            total_err = total_err.wrapping_add(compute_color_distance_rgb(&p, pixel, params.perceptual, &params.weights));
        }

        results.best_overall_err = total_err;

        (total_err, MODE_0_OPTIMAL_INDEX as i32)
    }

    /// Modes 6 and 7, which differ in their table and their selector weights. (Mode 7's colour
    /// is worked out from its six bits as they are, not widened to eight: as the original has
    /// it.)
    fn pack_mode67_to_one_color(
        table: &[[[EndpointErr; 2]; 2]; 256],
        weight: u32,
        params: &ColorCellCompressorParams,
        results: &mut ColorCellCompressorResults,
        r: u32,
        g: u32,
        b: u32,
        a: u32,
        pixels: &[ColorQuadI],
    ) -> u64 {
        let (r, g, b, a) = (r as usize, g as usize, b as usize, a as usize);

        let mut best_err = u32::MAX;
        let mut best_p = 0usize;

        for p in 0..4usize {
            let hi_p = p >> 1;
            let lo_p = p & 1;
            let err = table[r][hi_p][lo_p].error + table[g][hi_p][lo_p].error + table[b][hi_p][lo_p].error + table[a][hi_p][lo_p].error;
            if err < best_err {
                best_err = err;
                best_p = p;
            }
        }

        let best_hi_p = best_p >> 1;
        let best_lo_p = best_p & 1;

        let (er, eg, eb, ea) = (&table[r][best_hi_p][best_lo_p], &table[g][best_hi_p][best_lo_p], &table[b][best_hi_p][best_lo_p], &table[a][best_hi_p][best_lo_p]);

        results.low_endpoint = [er.lo as i32, eg.lo as i32, eb.lo as i32, ea.lo as i32];
        results.high_endpoint = [er.hi as i32, eg.hi as i32, eb.hi as i32, ea.hi as i32];

        results.pbits[0] = best_lo_p as u32;
        results.pbits[1] = best_hi_p as u32;

        let mut p = [0i32; 4];

        for i in 0..4 {
            let low = ((results.low_endpoint[i] as u32) << 1) | results.pbits[0];
            let high = ((results.high_endpoint[i] as u32) << 1) | results.pbits[1];

            p[i] = ((low * (64 - weight) + high * weight + 32) >> 6) as i32;
        }

        let mut total_err = 0u64;
        for pixel in pixels {
            total_err = total_err.wrapping_add(compute_color_distance_rgba(&p, pixel, params.perceptual, &params.weights));
        }

        results.best_overall_err = total_err;

        total_err
    }

    fn pack_mode6_to_one_color(&self, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, a: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        let total_err = Self::pack_mode67_to_one_color(&self.mode_6_optimal_endpoints, WEIGHTS4[MODE_6_OPTIMAL_INDEX as usize], params, results, r, g, b, a, pixels);
        (total_err, MODE_6_OPTIMAL_INDEX as i32)
    }

    fn pack_mode7_to_one_color(&self, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, a: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        let total_err = Self::pack_mode67_to_one_color(&self.mode_7_optimal_endpoints, WEIGHTS2[MODE_7_OPTIMAL_INDEX as usize], params, results, r, g, b, a, pixels);
        (total_err, MODE_7_OPTIMAL_INDEX as i32)
    }
}

/// The lowest of four errors and which it is, as the original picks it: of equal ones, the
/// last.
#[inline]
fn best_of_four(errs: [f32; 4]) -> (f32, i32) {
    let best_err = minimumf(minimumf(minimumf(errs[0], errs[1]), errs[2]), errs[3]);

    let mut best_sel = if best_err == errs[1] { 1 } else { 0 };
    if best_err == errs[2] {
        best_sel = 2;
    }
    if best_err == errs[3] {
        best_sel = 3;
    }

    (best_err, best_sel)
}

fn evaluate_solution(low: &ColorQuadI, high: &ColorQuadI, pbits: &[u32; 2], params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, pixels: &[ColorQuadI]) -> u64 {
    let num_pixels = pixels.len();

    let mut quant_min_color = *low;
    let mut quant_max_color = *high;

    if params.has_pbits {
        let (min_pbit, max_pbit);

        if params.endpoints_share_pbit {
            min_pbit = pbits[0];
            max_pbit = pbits[0];
        } else {
            min_pbit = pbits[0];
            max_pbit = pbits[1];
        }

        for c in 0..4 {
            quant_min_color[c] = (low[c] << 1) | min_pbit as i32;
            quant_max_color[c] = (high[c] << 1) | max_pbit as i32;
        }
    }

    let actual_min_color = scale_color(&quant_min_color, params);
    let actual_max_color = scale_color(&quant_max_color, params);

    let n = params.num_selector_weights as usize;
    let nc = if params.has_alpha { 4 } else { 3 };

    let mut total_errf = 0.0f32;

    let wr = params.weights[0] as f32;
    let mut wg = params.weights[1] as f32;
    let mut wb = params.weights[2] as f32;
    let wa = params.weights[3] as f32;

    let mut weighted_colors = [[0.0f32; 4]; 16];
    weighted_colors[0] = vec4f_from_color(&actual_min_color);
    weighted_colors[n - 1] = vec4f_from_color(&actual_max_color);

    for i in 1..(n - 1) {
        for j in 0..nc {
            weighted_colors[i][j] =
                fmath::floor((weighted_colors[0][j] * (64.0f32 - params.selector_weights[i] as f32) + weighted_colors[n - 1][j] * params.selector_weights[i] as f32 + 32.0f32) * (1.0f32 / 64.0f32));
        }
    }

    // A colour's error against a pixel, in RGB and in RGBA.
    let err_rgb = |color: &[f32; 4], pr: f32, pg: f32, pb: f32| {
        let dr = color[0] - pr;
        let dg = color[1] - pg;
        let db = color[2] - pb;
        wr * dr * dr + wg * dg * dg + wb * db * db
    };
    let err_rgba = |color: &[f32; 4], pr: f32, pg: f32, pb: f32, pa: f32| {
        let dr = color[0] - pr;
        let dg = color[1] - pg;
        let db = color[2] - pb;
        let da = color[3] - pa;
        wr * dr * dr + wg * dg * dg + wb * db * db + wa * da * da
    };

    if !params.perceptual {
        if !params.has_alpha {
            if n == 16 {
                let mut lr = actual_min_color[0] as f32;
                let mut lg = actual_min_color[1] as f32;
                let mut lb = actual_min_color[2] as f32;

                let dr = actual_max_color[0] as f32 - lr;
                let dg = actual_max_color[1] as f32 - lg;
                let db = actual_max_color[2] as f32 - lb;

                let f = n as f32 / (dr * dr + dg * dg + db * db);

                lr *= -dr;
                lg *= -dg;
                lb *= -db;

                for i in 0..num_pixels {
                    let c = &pixels[i];
                    let r = c[0] as f32;
                    let g = c[1] as f32;
                    let b = c[2] as f32;

                    let mut best_sel = fmath::floor(((r * dr + lr) + (g * dg + lg) + (b * db + lb)) * f + 0.5f32);
                    best_sel = clampf(best_sel, 1.0, (n - 1) as f32);

                    let best_sel0 = best_sel - 1.0;

                    let err0 = err_rgb(&weighted_colors[f2i(best_sel0) as usize], r, g, b);
                    let err1 = err_rgb(&weighted_colors[f2i(best_sel) as usize], r, g, b);

                    let min_err = minimumf(err0, err1);
                    total_errf += min_err;
                    results.selectors_temp[i] = f2i(if min_err == err0 { best_sel0 } else { best_sel });
                }
            } else if n == 8 {
                for i in 0..num_pixels {
                    let pr = pixels[i][0] as f32;
                    let pg = pixels[i][1] as f32;
                    let pb = pixels[i][2] as f32;

                    let (mut best_err, mut best_sel) = best_of_four(std::array::from_fn(|k| err_rgb(&weighted_colors[k], pr, pg, pb)));

                    let errs: [f32; 4] = std::array::from_fn(|k| err_rgb(&weighted_colors[4 + k], pr, pg, pb));
                    best_err = minimumf(best_err, minimumf(minimumf(minimumf(errs[0], errs[1]), errs[2]), errs[3]));
                    for k in 0..4 {
                        if best_err == errs[k] {
                            best_sel = 4 + k as i32;
                        }
                    }

                    total_errf += best_err;

                    results.selectors_temp[i] = best_sel;
                }
            } else {
                // if (N == 4)
                for i in 0..num_pixels {
                    let pr = pixels[i][0] as f32;
                    let pg = pixels[i][1] as f32;
                    let pb = pixels[i][2] as f32;

                    let (best_err, best_sel) = best_of_four(std::array::from_fn(|k| err_rgb(&weighted_colors[k], pr, pg, pb)));

                    total_errf += best_err;

                    results.selectors_temp[i] = best_sel;
                }
            }
        } else {
            // alpha
            if n == 16 {
                let mut lr = actual_min_color[0] as f32;
                let mut lg = actual_min_color[1] as f32;
                let mut lb = actual_min_color[2] as f32;
                let mut la = actual_min_color[3] as f32;

                let dr = actual_max_color[0] as f32 - lr;
                let dg = actual_max_color[1] as f32 - lg;
                let db = actual_max_color[2] as f32 - lb;
                let da = actual_max_color[3] as f32 - la;

                let f = n as f32 / (dr * dr + dg * dg + db * db + da * da);

                lr *= -dr;
                lg *= -dg;
                lb *= -db;
                la *= -da;

                for i in 0..num_pixels {
                    let c = &pixels[i];
                    let r = c[0] as f32;
                    let g = c[1] as f32;
                    let b = c[2] as f32;
                    let a = c[3] as f32;

                    let mut best_sel = fmath::floor(((r * dr + lr) + (g * dg + lg) + (b * db + lb) + (a * da + la)) * f + 0.5f32);
                    best_sel = clampf(best_sel, 1.0, (n - 1) as f32);

                    let best_sel0 = best_sel - 1.0;

                    let err0 = err_rgba(&weighted_colors[f2i(best_sel0) as usize], r, g, b, a);
                    let err1 = err_rgba(&weighted_colors[f2i(best_sel) as usize], r, g, b, a);

                    let min_err = minimumf(err0, err1);
                    total_errf += min_err;
                    results.selectors_temp[i] = f2i(if min_err == err0 { best_sel0 } else { best_sel });
                }
            } else if n == 8 {
                for i in 0..num_pixels {
                    let pr = pixels[i][0] as f32;
                    let pg = pixels[i][1] as f32;
                    let pb = pixels[i][2] as f32;
                    let pa = pixels[i][3] as f32;

                    let (mut best_err, mut best_sel) = best_of_four(std::array::from_fn(|k| err_rgba(&weighted_colors[k], pr, pg, pb, pa)));

                    let errs: [f32; 4] = std::array::from_fn(|k| err_rgba(&weighted_colors[4 + k], pr, pg, pb, pa));
                    best_err = minimumf(best_err, minimumf(minimumf(minimumf(errs[0], errs[1]), errs[2]), errs[3]));
                    for k in 0..4 {
                        if best_err == errs[k] {
                            best_sel = 4 + k as i32;
                        }
                    }

                    total_errf += best_err;

                    results.selectors_temp[i] = best_sel;
                }
            } else {
                // if (N == 4)
                for i in 0..num_pixels {
                    let pr = pixels[i][0] as f32;
                    let pg = pixels[i][1] as f32;
                    let pb = pixels[i][2] as f32;
                    let pa = pixels[i][3] as f32;

                    let (best_err, best_sel) = best_of_four(std::array::from_fn(|k| err_rgba(&weighted_colors[k], pr, pg, pb, pa)));

                    total_errf += best_err;

                    results.selectors_temp[i] = best_sel;
                }
            }
        }
    } else {
        wg *= PR_WEIGHT;
        wb *= PB_WEIGHT;

        let mut weighted_colors_y = [0.0f32; 16];
        let mut weighted_colors_cr = [0.0f32; 16];
        let mut weighted_colors_cb = [0.0f32; 16];

        for i in 0..n {
            let r = weighted_colors[i][0];
            let g = weighted_colors[i][1];
            let b = weighted_colors[i][2];

            let y = r * 0.2126f32 + g * 0.7152f32 + b * 0.0722f32;

            weighted_colors_y[i] = y;
            weighted_colors_cr[i] = r - y;
            weighted_colors_cb[i] = b - y;
        }

        for i in 0..num_pixels {
            let r = pixels[i][0] as f32;
            let g = pixels[i][1] as f32;
            let b = pixels[i][2] as f32;
            let a = pixels[i][3] as f32;

            let y = r * 0.2126f32 + g * 0.7152f32 + b * 0.0722f32;
            let cr = r - y;
            let cb = b - y;

            let mut best_err = 1e+10f32;
            let mut best_sel = 0i32;
            for j in 0..n {
                let dl = y - weighted_colors_y[j];
                let dcr = cr - weighted_colors_cr[j];
                let dcb = cb - weighted_colors_cb[j];

                let err = if params.has_alpha {
                    let da = a - weighted_colors[j][3];
                    (wr * dl * dl) + (wg * dcr * dcr) + (wb * dcb * dcb) + (wa * da * da)
                } else {
                    (wr * dl * dl) + (wg * dcr * dcr) + (wb * dcb * dcb)
                };
                if err < best_err {
                    best_err = err;
                    best_sel = j as i32;
                }
            }

            total_errf += best_err;

            results.selectors_temp[i] = best_sel;
        }
    }

    let total_err = total_errf as i64 as u64;

    if total_err < results.best_overall_err {
        results.best_overall_err = total_err;

        results.low_endpoint = *low;
        results.high_endpoint = *high;

        results.pbits[0] = pbits[0];
        results.pbits[1] = pbits[1];

        results.selectors[..num_pixels].copy_from_slice(&results.selectors_temp[..num_pixels]);
    }

    total_err
}

fn fix_degenerate_endpoints(mode: u32, trial_min_color: &mut ColorQuadI, trial_max_color: &mut ColorQuadI, xl: &Vec4F, xh: &Vec4F, iscale: u32) {
    if mode == 1 || mode == 4 {
        // also mode 2
        // fix degenerate case where the input collapses to a single colorspace voxel, and we loose all freedom (test with grayscale ramps)
        let iscale = iscale as i32;
        for i in 0..3 {
            if trial_min_color[i] == trial_max_color[i] {
                if (xl[i] - xh[i]).abs() > 0.0 {
                    if trial_min_color[i] > (iscale >> 1) {
                        if trial_min_color[i] > 0 {
                            trial_min_color[i] -= 1;
                        } else if trial_max_color[i] < iscale {
                            trial_max_color[i] += 1;
                        }
                    } else {
                        if trial_max_color[i] < iscale {
                            trial_max_color[i] += 1;
                        } else if trial_min_color[i] > 0 {
                            trial_min_color[i] -= 1;
                        }
                    }

                    if mode == 4 {
                        if trial_min_color[i] > (iscale >> 1) {
                            if trial_max_color[i] < iscale {
                                trial_max_color[i] += 1;
                            } else if trial_min_color[i] > 0 {
                                trial_min_color[i] -= 1;
                            }
                        } else {
                            if trial_min_color[i] > 0 {
                                trial_min_color[i] -= 1;
                            } else if trial_max_color[i] < iscale {
                                trial_max_color[i] += 1;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The endpoints nearest `xl` and `xh` among those with p-bit `p`, the p-bit still in them.
#[inline]
fn round_to_pbit(xl: &Vec4F, xh: &Vec4F, p: i32, iscalep: i32) -> (ColorQuadI, ColorQuadI) {
    let scalep = iscalep as f32;
    let mut x_min_color = [0i32; 4];
    let mut x_max_color = [0i32; 4];
    for c in 0..4 {
        x_min_color[c] = f2i((xl[c] * scalep - p as f32) / 2.0 + 0.5).wrapping_mul(2).wrapping_add(p);
        x_min_color[c] = clampi(x_min_color[c], p, iscalep - 1 + p);

        x_max_color[c] = f2i((xh[c] * scalep - p as f32) / 2.0 + 0.5).wrapping_mul(2).wrapping_add(p);
        x_max_color[c] = clampi(x_max_color[c], p, iscalep - 1 + p);
    }
    (x_min_color, x_max_color)
}

fn find_optimal_solution(mode: u32, xl: &Vec4F, xh: &Vec4F, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, pbit_search: bool, pixels: &[ColorQuadI]) -> u64 {
    let xl = vec4f_saturate(xl);
    let xh = vec4f_saturate(xh);

    if params.has_pbits {
        // Notes: The pbit controls which quantization intervals are selected.
        // total_levels=2^(comp_bits+1), where comp_bits=4 for mode 0, etc.
        // pbit 0: v=(b*2)/(total_levels-1), pbit 1: v=(b*2+1)/(total_levels-1) where b is the component bin from [0,total_levels/2-1] and v is the [0,1] component value
        // rearranging you get for pbit 0: b=floor(v*(total_levels-1)/2+.5)
        // rearranging you get for pbit 1: b=floor((v*(total_levels-1)-1)/2+.5)
        let iscalep = (1i32 << (params.comp_bits + 1)) - 1;
        let total_comps = if params.has_alpha { 4 } else { 3 };

        if pbit_search {
            // compensated rounding+pbit search
            let mut lo = [[0i32; 4]; 2];
            let mut hi = [[0i32; 4]; 2];

            for p in 0..2 {
                let (x_min_color, x_max_color) = round_to_pbit(&xl, &xh, p as i32, iscalep);

                lo[p] = x_min_color;
                hi[p] = x_max_color;

                for c in 0..4 {
                    lo[p][c] >>= 1;
                    hi[p][c] >>= 1;
                }
            }

            fix_degenerate_endpoints(mode, &mut lo[0], &mut hi[0], &xl, &xh, (iscalep >> 1) as u32);
            fix_degenerate_endpoints(mode, &mut lo[1], &mut hi[1], &xl, &xh, (iscalep >> 1) as u32);

            if !params.endpoints_share_pbit {
                evaluate_solution(&lo[0], &hi[0], &[0, 0], params, results, pixels);
                evaluate_solution(&lo[0], &hi[1], &[0, 1], params, results, pixels);
                evaluate_solution(&lo[1], &hi[0], &[1, 0], params, results, pixels);
                evaluate_solution(&lo[1], &hi[1], &[1, 1], params, results, pixels);
            } else {
                // Endpoints share pbits
                evaluate_solution(&lo[0], &hi[0], &[0, 0], params, results, pixels);
                evaluate_solution(&lo[1], &hi[1], &[1, 1], params, results, pixels);
            }
        } else {
            // compensated rounding
            let mut best_pbits = [0u32; 2];
            let mut best_min_color = [0i32; 4];
            let mut best_max_color = [0i32; 4];

            if !params.endpoints_share_pbit {
                let mut best_err0 = 1e+9f32;
                let mut best_err1 = 1e+9f32;

                for p in 0..2i32 {
                    let (x_min_color, x_max_color) = round_to_pbit(&xl, &xh, p, iscalep);

                    let scaled_low = scale_color(&x_min_color, params);
                    let scaled_high = scale_color(&x_max_color, params);

                    let mut err0 = 0.0f32;
                    let mut err1 = 0.0f32;
                    for i in 0..total_comps {
                        err0 += squaref(scaled_low[i] as f32 - xl[i] * 255.0);
                        err1 += squaref(scaled_high[i] as f32 - xh[i] * 255.0);
                    }

                    if err0 < best_err0 {
                        best_err0 = err0;
                        best_pbits[0] = p as u32;

                        for c in 0..4 {
                            best_min_color[c] = x_min_color[c] >> 1;
                        }
                    }

                    if err1 < best_err1 {
                        best_err1 = err1;
                        best_pbits[1] = p as u32;

                        for c in 0..4 {
                            best_max_color[c] = x_max_color[c] >> 1;
                        }
                    }
                }
            } else {
                // Endpoints share pbits
                let mut best_err = 1e+9f32;

                for p in 0..2i32 {
                    let (x_min_color, x_max_color) = round_to_pbit(&xl, &xh, p, iscalep);

                    let scaled_low = scale_color(&x_min_color, params);
                    let scaled_high = scale_color(&x_max_color, params);

                    let mut err = 0.0f32;
                    for i in 0..total_comps {
                        err += squaref((scaled_low[i] as f32 / 255.0) - xl[i]) + squaref((scaled_high[i] as f32 / 255.0) - xh[i]);
                    }

                    if err < best_err {
                        best_err = err;
                        best_pbits[0] = p as u32;
                        best_pbits[1] = p as u32;
                        for c in 0..4 {
                            best_min_color[c] = x_min_color[c] >> 1;
                            best_max_color[c] = x_max_color[c] >> 1;
                        }
                    }
                }
            }

            fix_degenerate_endpoints(mode, &mut best_min_color, &mut best_max_color, &xl, &xh, (iscalep >> 1) as u32);

            if results.best_overall_err == u64::MAX
                || best_min_color != results.low_endpoint
                || best_max_color != results.high_endpoint
                || best_pbits[0] != results.pbits[0]
                || best_pbits[1] != results.pbits[1]
            {
                evaluate_solution(&best_min_color, &best_max_color, &best_pbits, params, results, pixels);
            }
        }
    } else {
        let iscale = (1i32 << params.comp_bits) - 1;
        let scale = iscale as f32;

        let mut trial_min_color = color_quad_i_set_clamped(f2i(xl[0] * scale + 0.5), f2i(xl[1] * scale + 0.5), f2i(xl[2] * scale + 0.5), f2i(xl[3] * scale + 0.5));
        let mut trial_max_color = color_quad_i_set_clamped(f2i(xh[0] * scale + 0.5), f2i(xh[1] * scale + 0.5), f2i(xh[2] * scale + 0.5), f2i(xh[3] * scale + 0.5));

        fix_degenerate_endpoints(mode, &mut trial_min_color, &mut trial_max_color, &xl, &xh, iscale as u32);

        if results.best_overall_err == u64::MAX || trial_min_color != results.low_endpoint || trial_max_color != results.high_endpoint {
            evaluate_solution(&trial_min_color, &trial_max_color, &[0, 0], params, results, pixels);
        }
    }

    results.best_overall_err
}

impl Bc7e {
    /// A subset packed as one colour by the table of its mode.
    fn pack_to_one_color(&self, mode: u32, params: &ColorCellCompressorParams, results: &mut ColorCellCompressorResults, r: u32, g: u32, b: u32, a: u32, pixels: &[ColorQuadI]) -> (u64, i32) {
        if mode == 0 {
            self.pack_mode0_to_one_color(params, results, r, g, b, pixels)
        } else if mode == 1 {
            self.pack_mode1_to_one_color(params, results, r, g, b, pixels)
        } else if mode == 6 {
            self.pack_mode6_to_one_color(params, results, r, g, b, a, pixels)
        } else if mode == 7 {
            self.pack_mode7_to_one_color(params, results, r, g, b, a, pixels)
        } else {
            self.pack_mode24_to_one_color(params, results, r, g, b, pixels)
        }
    }

    // Note: In mode 6, m_has_alpha will only be true for transparent blocks.
    fn color_cell_compression(
        &self,
        mode: u32,
        params: &ColorCellCompressorParams,
        results: &mut ColorCellCompressorResults,
        comp_params: &CompressBlockParams,
        pixels: &[ColorQuadI],
        refinement: bool,
    ) -> u64 {
        let num_pixels = pixels.len();

        results.best_overall_err = u64::MAX;

        if mode != 6 && mode != 7 {
            debug_assert!(!params.has_alpha);
        }

        if mode <= 2 || mode == 4 || mode >= 6 {
            let first = pixels[0];

            let all_same = pixels[1..].iter().all(|pixel| *pixel == first);

            if all_same {
                let (total_err, index) = self.pack_to_one_color(mode, params, results, first[0] as u32, first[1] as u32, first[2] as u32, first[3] as u32, pixels);
                results.selectors[..num_pixels].fill(index);
                return total_err;
            }
        }

        let mut mean_color = [0.0f32; 4];
        let mut axis;

        for pixel in pixels {
            let color = vec4f_from_color(pixel);
            mean_color = vec4f_add(&mean_color, &color);
        }

        let mean_color_scaled = vec4f_mul(&mean_color, 1.0 / (num_pixels as i32 as f32));

        mean_color = vec4f_mul(&mean_color, 1.0 / (num_pixels as i32 as f32 * 255.0));
        mean_color = vec4f_saturate(&mean_color);

        if params.has_alpha {
            let mut v = [0.0f32; 4];
            for (i, pixel) in pixels.iter().enumerate() {
                let mut color = vec4f_from_color(pixel);
                color = vec4f_sub(&color, &mean_color_scaled);

                let a = vec4f_mul(&color, color[0]);
                let b = vec4f_mul(&color, color[1]);
                let c = vec4f_mul(&color, color[2]);
                let d = vec4f_mul(&color, color[3]);

                let mut n = if i != 0 { v } else { color };
                vec4f_normalize_in_place(&mut n);

                v[0] += vec4f_dot(&a, &n);
                v[1] += vec4f_dot(&b, &n);
                v[2] += vec4f_dot(&c, &n);
                v[3] += vec4f_dot(&d, &n);
            }
            axis = v;
            vec4f_normalize_in_place(&mut axis);
        } else {
            let mut cov = [0.0f32; 6];

            for pixel in pixels {
                let r = pixel[0] as f32 - mean_color_scaled[0];
                let g = pixel[1] as f32 - mean_color_scaled[1];
                let b = pixel[2] as f32 - mean_color_scaled[2];

                cov[0] += r * r;
                cov[1] += r * g;
                cov[2] += r * b;
                cov[3] += g * g;
                cov[4] += g * b;
                cov[5] += b * b;
            }

            // This is more stable.
            let (mut vfr, mut vfg, mut vfb) = (0.9f32, 1.0f32, 0.7f32);

            for _ in 0..3 {
                let mut r = vfr * cov[0] + vfg * cov[1] + vfb * cov[2];
                let mut g = vfr * cov[1] + vfg * cov[3] + vfb * cov[4];
                let mut b = vfr * cov[2] + vfg * cov[4] + vfb * cov[5];

                let mut m = maximumf(maximumf(r.abs(), g.abs()), b.abs());
                if m > 1e-10f32 {
                    m = 1.0 / m;
                    r *= m;
                    g *= m;
                    b *= m;
                }

                vfr = r;
                vfg = g;
                vfb = b;
            }

            let mut len = vfr * vfr + vfg * vfg + vfb * vfb;

            if len < 1e-10f32 {
                axis = [0.0; 4];
            } else {
                len = 1.0 / len.sqrt();
                vfr *= len;
                vfg *= len;
                vfb *= len;
                axis = [vfr, vfg, vfb, 0.0];
            }
        }

        if vec4f_dot(&axis, &axis) < 0.5 {
            if params.perceptual {
                axis = [0.213, 0.715, 0.072, if params.has_alpha { 0.715 } else { 0.0 }];
            } else {
                axis = [1.0, 1.0, 1.0, if params.has_alpha { 1.0 } else { 0.0 }];
            }
            vec4f_normalize_in_place(&mut axis);
        }

        let (mut l, mut h) = (1e+9f32, -1e+9f32);

        for pixel in pixels {
            let color = vec4f_from_color(pixel);

            let q = vec4f_sub(&color, &mean_color_scaled);
            let d = vec4f_dot(&q, &axis);

            l = minimumf(l, d);
            h = maximumf(h, d);
        }

        l *= 1.0f32 / 255.0f32;
        h *= 1.0f32 / 255.0f32;

        let b0 = vec4f_mul(&axis, l);
        let b1 = vec4f_mul(&axis, h);
        let c0 = vec4f_add(&mean_color, &b0);
        let c1 = vec4f_add(&mean_color, &b1);
        let mut min_color = vec4f_saturate(&c0);
        let mut max_color = vec4f_saturate(&c1);

        let white_vec = [1.0f32; 4];

        if vec4f_dot(&min_color, &white_vec) > vec4f_dot(&max_color, &white_vec) {
            std::mem::swap(&mut min_color, &mut max_color);
        }

        if find_optimal_solution(mode, &min_color, &max_color, params, results, comp_params.pbit_search, pixels) == 0 {
            return 0;
        }

        if !refinement {
            return results.best_overall_err;
        }

        // The endpoints that fit the selectors best, as find_optimal_solution takes them.
        let least_squares = |selectors: &[i32]| -> (Vec4F, Vec4F) {
            let mut xl = [0.0f32; 4];
            let mut xh = [0.0f32; 4];
            if params.has_alpha {
                compute_least_squares_endpoints_rgba(num_pixels, selectors, params.selector_weightsx, &mut xl, &mut xh, pixels);
            } else {
                compute_least_squares_endpoints_rgb(num_pixels, selectors, params.selector_weightsx, &mut xl, &mut xh, pixels);

                xl[3] = 255.0;
                xh[3] = 255.0;
            }

            (vec4f_mul(&xl, 1.0f32 / 255.0f32), vec4f_mul(&xh, 1.0f32 / 255.0f32))
        };

        for _ in 0..comp_params.refinement_passes {
            let (xl, xh) = least_squares(&results.selectors);
            if find_optimal_solution(mode, &xl, &xh, params, results, comp_params.pbit_search, pixels) == 0 {
                return 0;
            }
        }

        if comp_params.uber_level > 0 {
            let mut selectors_temp = [0i32; 16];
            let mut selectors_temp1 = [0i32; 16];
            selectors_temp[..num_pixels].copy_from_slice(&results.selectors[..num_pixels]);

            let max_selector = params.num_selector_weights as i32 - 1;

            let mut min_sel = 16u32;
            let mut max_sel = 0u32;
            for i in 0..num_pixels {
                let sel = selectors_temp[i] as u32;
                min_sel = min_sel.min(sel);
                max_sel = max_sel.max(sel);
            }

            if comp_params.uber1_mask & 1 != 0 {
                for i in 0..num_pixels {
                    let mut sel = selectors_temp[i] as u32;
                    if sel == min_sel && sel < (params.num_selector_weights - 1) {
                        sel += 1;
                    }
                    selectors_temp1[i] = sel as i32;
                }

                let (xl, xh) = least_squares(&selectors_temp1);
                if find_optimal_solution(mode, &xl, &xh, params, results, comp_params.pbit_search, pixels) == 0 {
                    return 0;
                }
            }

            if comp_params.uber1_mask & 2 != 0 {
                for i in 0..num_pixels {
                    let mut sel = selectors_temp[i] as u32;
                    if sel == max_sel && sel > 0 {
                        sel -= 1;
                    }
                    selectors_temp1[i] = sel as i32;
                }

                let (xl, xh) = least_squares(&selectors_temp1);
                if find_optimal_solution(mode, &xl, &xh, params, results, comp_params.pbit_search, pixels) == 0 {
                    return 0;
                }
            }

            if comp_params.uber1_mask & 4 != 0 {
                for i in 0..num_pixels {
                    let mut sel = selectors_temp[i] as u32;
                    if sel == min_sel && sel < (params.num_selector_weights - 1) {
                        sel += 1;
                    } else if sel == max_sel && sel > 0 {
                        sel -= 1;
                    }
                    selectors_temp1[i] = sel as i32;
                }

                let (xl, xh) = least_squares(&selectors_temp1);
                if find_optimal_solution(mode, &xl, &xh, params, results, comp_params.pbit_search, pixels) == 0 {
                    return 0;
                }
            }

            let uber_err_thresh = (num_pixels as u32 * 56) >> 4;
            if comp_params.uber_level >= 2 && results.best_overall_err > uber_err_thresh as u64 {
                let q = if comp_params.uber_level >= 4 { (comp_params.uber_level - 2) as i32 } else { 1 };
                for ly in -q..=1 {
                    for hy in (max_selector - 1)..=(max_selector + q) {
                        if ly == 0 && hy == max_selector {
                            continue;
                        }

                        for i in 0..num_pixels {
                            selectors_temp1[i] = f2i(clampf(fmath::floor(max_selector as f32 * (selectors_temp[i] as f32 - ly as f32) / (hy as f32 - ly as f32) + 0.5), 0.0, max_selector as f32));
                        }

                        let (xl, xh) = least_squares(&selectors_temp1);
                        if find_optimal_solution(mode, &xl, &xh, params, results, comp_params.pbit_search && comp_params.uber_level >= 2, pixels) == 0 {
                            return 0;
                        }
                    }
                }
            }
        }

        if mode <= 2 || mode == 4 || mode >= 6 {
            let mut avg_results = *results;

            let r = f2i(0.5 + mean_color[0] * 255.0) as u32;
            let g = f2i(0.5 + mean_color[1] * 255.0) as u32;
            let b = f2i(0.5 + mean_color[2] * 255.0) as u32;
            let a = f2i(0.5 + mean_color[3] * 255.0) as u32;

            let (avg_err, index) = self.pack_to_one_color(mode, params, &mut avg_results, r, g, b, a, pixels);

            if avg_err < results.best_overall_err {
                results.best_overall_err = avg_err;
                results.low_endpoint = avg_results.low_endpoint;
                results.high_endpoint = avg_results.high_endpoint;
                results.pbits = avg_results.pbits;

                results.selectors[..num_pixels].fill(index);
            }
        }

        results.best_overall_err
    }
}

fn color_cell_compression_est(mode: u32, params: &ColorCellCompressorParams, pixels: &[ColorQuadI]) -> u64 {
    debug_assert!(params.num_selector_weights == 4 || params.num_selector_weights == 8);

    let (mut lr, mut lg, mut lb) = (255.0f32, 255.0f32, 255.0f32);
    let (mut hr, mut hg, mut hb) = (0.0f32, 0.0f32, 0.0f32);
    for c in pixels {
        let r = c[0] as f32;
        let g = c[1] as f32;
        let b = c[2] as f32;

        lr = minimumf(lr, r);
        lg = minimumf(lg, g);
        lb = minimumf(lb, b);

        hr = maximumf(hr, r);
        hg = maximumf(hg, g);
        hb = maximumf(hb, b);
    }

    let n = 1u32 << COLOR_INDEX_BITCOUNT[mode as usize];

    let sr = lr;
    let sg = lg;
    let sb = lb;

    let dir = hr - lr;
    let dig = hg - lg;
    let dib = hb - lb;

    let far = dir;
    let fag = dig;
    let fab = dib;

    let low = far * sr + fag * sg + fab * sb;
    let high = far * hr + fag * hg + fab * hb;

    let scale = (n as f32 - 1.0) / (high - low);
    let inv_n = 1.0 / (n as f32 - 1.0);

    let mut total_errf = 0.0f32;

    // We don't handle perceptual very well here, but the difference is very slight (<.05 dB avg Luma PSNR across a large corpus) and the perf lost was high (2x slower).
    if params.weights[0] != 1 || params.weights[1] != 1 || params.weights[2] != 1 {
        let wr = params.weights[0] as f32;
        let wg = params.weights[1] as f32;
        let wb = params.weights[2] as f32;

        for c in pixels {
            let d = far * c[0] as f32 + fag * c[1] as f32 + fab * c[2] as f32;

            let s = clampf(fmath::floor((d - low) * scale + 0.5) * inv_n, 0.0, 1.0);

            let itr = sr + dir * s;
            let itg = sg + dig * s;
            let itb = sb + dib * s;

            let dr = itr - c[0] as f32;
            let dg = itg - c[1] as f32;
            let db = itb - c[2] as f32;

            total_errf += wr * dr * dr + wg * dg * dg + wb * db * db;
        }
    } else {
        for c in pixels {
            let d = far * c[0] as f32 + fag * c[1] as f32 + fab * c[2] as f32;

            let s = clampf(fmath::floor((d - low) * scale + 0.5) * inv_n, 0.0, 1.0);

            let itr = sr + dir * s;
            let itg = sg + dig * s;
            let itb = sb + dib * s;

            let dr = itr - c[0] as f32;
            let dg = itg - c[1] as f32;
            let db = itb - c[2] as f32;

            total_errf += dr * dr + dg * dg + db * db;
        }
    }

    total_errf as i64 as u64
}

fn color_cell_compression_est_mode7(mode: u32, params: &ColorCellCompressorParams, pixels: &[ColorQuadI]) -> u64 {
    debug_assert!(mode == 7 && params.num_selector_weights == 4);

    let (mut lr, mut lg, mut lb, mut la) = (255.0f32, 255.0f32, 255.0f32, 255.0f32);
    let (mut hr, mut hg, mut hb, mut ha) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for c in pixels {
        let r = c[0] as f32;
        let g = c[1] as f32;
        let b = c[2] as f32;
        let a = c[3] as f32;

        lr = minimumf(lr, r);
        lg = minimumf(lg, g);
        lb = minimumf(lb, b);
        la = minimumf(la, a);

        hr = maximumf(hr, r);
        hg = maximumf(hg, g);
        hb = maximumf(hb, b);
        ha = maximumf(ha, a);
    }

    let n = 4u32;

    let sr = lr;
    let sg = lg;
    let sb = lb;
    let sa = la;

    let dir = hr - lr;
    let dig = hg - lg;
    let dib = hb - lb;
    let dia = ha - la;

    let far = dir;
    let fag = dig;
    let fab = dib;
    let faa = dia;

    let low = far * sr + fag * sg + fab * sb + faa * sa;
    let high = far * hr + fag * hg + fab * hb + faa * ha;

    let scale = (n as f32 - 1.0) / (high - low);
    let inv_n = 1.0 / (n as f32 - 1.0);

    let mut total_errf = 0.0f32;

    // We don't handle perceptual very well here, but the difference is very slight (<.05 dB avg Luma PSNR across a large corpus) and the perf lost was high (2x slower).
    if !params.perceptual && (params.weights[0] != 1 || params.weights[1] != 1 || params.weights[2] != 1 || params.weights[3] != 1) {
        let wr = params.weights[0] as f32;
        let wg = params.weights[1] as f32;
        let wb = params.weights[2] as f32;
        let wa = params.weights[3] as f32;

        for c in pixels {
            let d = far * c[0] as f32 + fag * c[1] as f32 + fab * c[2] as f32 + faa * c[3] as f32;

            let s = clampf(fmath::floor((d - low) * scale + 0.5) * inv_n, 0.0, 1.0);

            let itr = sr + dir * s;
            let itg = sg + dig * s;
            let itb = sb + dib * s;
            let ita = sa + dia * s;

            let dr = itr - c[0] as f32;
            let dg = itg - c[1] as f32;
            let db = itb - c[2] as f32;
            let da = ita - c[3] as f32;

            total_errf += wr * dr * dr + wg * dg * dg + wb * db * db + wa * da * da;
        }
    } else {
        for c in pixels {
            let d = far * c[0] as f32 + fag * c[1] as f32 + fab * c[2] as f32 + faa * c[3] as f32;

            let s = clampf(fmath::floor((d - low) * scale + 0.5) * inv_n, 0.0, 1.0);

            let itr = sr + dir * s;
            let itg = sg + dig * s;
            let itb = sb + dib * s;
            let ita = sa + dia * s;

            let dr = itr - c[0] as f32;
            let dg = itg - c[1] as f32;
            let db = itb - c[2] as f32;
            let da = ita - c[3] as f32;

            total_errf += dr * dr + dg * dg + db * db + da * da;
        }
    }

    total_errf as i64 as u64
}

/// A block's pixels dealt out to the subsets of a partition: each subset's pixels, how many
/// it has, and where in the block each came from.
struct Subsets {
    colors: [[ColorQuadI; 16]; 3],
    total_colors: [usize; 3],
    pixel_index: [[usize; 16]; 3],
}

impl Subsets {
    fn new(pixels: &[ColorQuadI; 16], total_subsets: u32, partition: u32) -> Self {
        let at = partition as usize * 16;
        let table = if total_subsets == 3 { &PARTITION3[at..at + 16] } else { &PARTITION2[at..at + 16] };
        let mut subsets = Subsets { colors: [[[0; 4]; 16]; 3], total_colors: [0; 3], pixel_index: [[0; 16]; 3] };
        for index in 0..16 {
            let p = table[index] as usize;
            subsets.colors[p][subsets.total_colors[p]] = pixels[index];
            subsets.pixel_index[p][subsets.total_colors[p]] = index;
            subsets.total_colors[p] += 1;
        }
        subsets
    }

    fn pixels(&self, subset: usize) -> &[ColorQuadI] {
        &self.colors[subset][..self.total_colors[subset]]
    }
}

/// What the estimators pack a subset with.
fn estimator_params(mode: u32, comp_params: &CompressBlockParams) -> ColorCellCompressorParams {
    let mut params = ColorCellCompressorParams::new();

    params.selector_weights = if COLOR_INDEX_BITCOUNT[mode as usize] == 2 { &WEIGHTS2 } else { &WEIGHTS3 };
    params.num_selector_weights = 1 << COLOR_INDEX_BITCOUNT[mode as usize];

    params.weights = comp_params.weights;
    if mode >= 6 {
        for c in 0..4 {
            params.weights[c] = params.weights[c].wrapping_mul(comp_params.alpha_settings.mode67_error_weight_mul[c]);
        }
    }

    params.perceptual = comp_params.perceptual;
    params
}

/// The estimated error of a partition: each of its subsets through the estimator.
fn estimate_partition_err(mode: u32, total_subsets: u32, params: &ColorCellCompressorParams, pixels: &[ColorQuadI; 16], partition: u32) -> u64 {
    let subsets = Subsets::new(pixels, total_subsets, partition);

    let mut total_subset_err = 0u64;

    for subset in 0..total_subsets as usize {
        let err = if mode == 7 { color_cell_compression_est_mode7(mode, params, subsets.pixels(subset)) } else { color_cell_compression_est(mode, params, subsets.pixels(subset)) };

        total_subset_err = total_subset_err.wrapping_add(err);
    }

    total_subset_err
}

fn estimate_partition(mode: u32, pixels: &[ColorQuadI; 16], comp_params: &CompressBlockParams) -> u32 {
    let total_subsets = NUM_SUBSETS[mode as usize];
    let total_partitions = comp_params.max_partitions_mode[mode as usize].min(1 << PARTITION_BITS[mode as usize]);

    if total_partitions <= 1 {
        return 0;
    }

    let mut best_err = u64::MAX;
    let mut best_partition = 0u32;

    let params = estimator_params(mode, comp_params);

    for partition in 0..total_partitions {
        let total_subset_err = estimate_partition_err(mode, total_subsets, &params, pixels, partition);

        if total_subset_err < best_err {
            best_err = total_subset_err;
            best_partition = partition;
            if best_err == 0 {
                break;
            }
        }

        if total_subsets == 2 {
            if partition == CHECKERBOARD_PARTITION_INDEX_2SUBSET && best_partition != CHECKERBOARD_PARTITION_INDEX_2SUBSET {
                break;
            }
        }
    }

    best_partition
}

/// A partition and its estimated error: `solution`.
#[derive(Clone, Copy, Default)]
struct Solution {
    index: u32,
    err: u64,
}

/// The partitions with the lowest estimated error, best first, into `solutions`; how many.
fn estimate_partition_list(mode: u32, pixels: &[ColorQuadI; 16], comp_params: &CompressBlockParams, solutions: &mut [Solution], max_solutions: i32) -> u32 {
    let orig_max_solutions = max_solutions;
    let mut max_solutions = max_solutions;

    let total_subsets = NUM_SUBSETS[mode as usize];
    let total_partitions = comp_params.max_partitions_mode[mode as usize].min(1 << PARTITION_BITS[mode as usize]);

    if total_partitions <= 1 {
        solutions[0].index = 0;
        solutions[0].err = 0;
        return 1;
    } else if max_solutions as u32 >= total_partitions {
        for i in 0..total_partitions {
            solutions[i as usize].index = i;
            solutions[i as usize].err = i as u64;
        }
        return total_partitions;
    }

    const HIGH_FREQUENCY_SORTED_PARTITION_THRESHOLD: i32 = 4;
    if total_subsets == 2 {
        if max_solutions < HIGH_FREQUENCY_SORTED_PARTITION_THRESHOLD {
            max_solutions = HIGH_FREQUENCY_SORTED_PARTITION_THRESHOLD;
        }
    }

    let params = estimator_params(mode, comp_params);

    let mut num_solutions = 0i32;

    for partition in 0..total_partitions {
        let total_subset_err = estimate_partition_err(mode, total_subsets, &params, pixels, partition);

        let mut i = 0i32;
        while i < num_solutions {
            if total_subset_err < solutions[i as usize].err {
                break;
            }
            i += 1;
        }

        if i < num_solutions {
            let mut solutions_to_move = (max_solutions - 1) - i;
            let num_elements_at_i = num_solutions - i;
            if solutions_to_move > num_elements_at_i {
                solutions_to_move = num_elements_at_i;
            }

            debug_assert!(((i + 1) + solutions_to_move) <= max_solutions);
            debug_assert!((i + solutions_to_move) <= num_solutions);

            let mut j = solutions_to_move - 1;
            while j >= 0 {
                solutions[(i + j + 1) as usize] = solutions[(i + j) as usize];
                j -= 1;
            }
        }

        if num_solutions < max_solutions {
            num_solutions += 1;
        }

        if i < num_solutions {
            solutions[i as usize].err = total_subset_err;
            solutions[i as usize].index = partition;
        }
    }

    num_solutions.min(orig_max_solutions) as u32
}

#[inline]
fn set_block_bits(bytes: &mut [u8; 16], mut val: u32, mut num_bits: u32, cur_ofs: &mut u32) {
    debug_assert!(num_bits < 32);
    debug_assert!(val < (1u32 << num_bits));
    while num_bits != 0 {
        let n = (8 - (*cur_ofs & 7)).min(num_bits);
        bytes[(*cur_ofs >> 3) as usize] |= (val << (*cur_ofs & 7)) as u8;
        val >>= n;
        num_bits -= n;
        *cur_ofs += n;
    }
    debug_assert!(*cur_ofs <= 128);
}

/// How a block is to be packed: `bc7_optimization_results`.
#[derive(Clone, Copy)]
struct OptimizationResults {
    mode: u32,
    partition: u32,
    selectors: [i32; 16],
    alpha_selectors: [i32; 16],
    low: [ColorQuadI; 3],
    high: [ColorQuadI; 3],
    pbits: [[u32; 2]; 3],
    rotation: u32,
    index_selector: u32,
}

impl OptimizationResults {
    fn new() -> Self {
        OptimizationResults { mode: 0, partition: 0, selectors: [0; 16], alpha_selectors: [0; 16], low: [[0; 4]; 3], high: [[0; 4]; 3], pbits: [[0; 2]; 3], rotation: 0, index_selector: 0 }
    }
}

fn encode_bc7_block(block: &mut [u8; 16], results: &OptimizationResults) {
    let best_mode = results.mode;

    let total_subsets = NUM_SUBSETS[best_mode as usize];

    let total_partitions = 1u32 << PARTITION_BITS[best_mode as usize];

    let partition_at = results.partition as usize * 16;
    let partition: &[u8] = if total_subsets == 1 {
        &PARTITION1
    } else if total_subsets == 2 {
        &PARTITION2[partition_at..partition_at + 16]
    } else {
        &PARTITION3[partition_at..partition_at + 16]
    };

    let mut color_selectors = results.selectors;
    let mut alpha_selectors = results.alpha_selectors;

    let mut low = results.low;
    let mut high = results.high;

    let mut pbits = results.pbits;

    let mut anchor = [-1i32; 3];

    for k in 0..total_subsets as usize {
        let mut anchor_index = 0usize;
        if k != 0 {
            if total_subsets == 3 && k == 1 {
                anchor_index = ANCHOR_INDEX_THIRD_SUBSET_1[results.partition as usize] as usize;
            } else if total_subsets == 3 && k == 2 {
                anchor_index = ANCHOR_INDEX_THIRD_SUBSET_2[results.partition as usize] as usize;
            } else {
                anchor_index = ANCHOR_INDEX_SECOND_SUBSET[results.partition as usize] as usize;
            }
        }

        anchor[k] = anchor_index as i32;

        let color_index_bits = get_bc7_color_index_size(best_mode, results.index_selector);
        let num_color_indices = 1u32 << color_index_bits;

        if (color_selectors[anchor_index] as u32 & (num_color_indices >> 1)) != 0 {
            for i in 0..16 {
                if partition[i] as usize == k {
                    color_selectors[i] = (num_color_indices - 1).wrapping_sub(color_selectors[i] as u32) as i32;
                }
            }

            if get_bc7_mode_has_seperate_alpha_selectors(best_mode) {
                for q in 0..3 {
                    std::mem::swap(&mut low[k][q], &mut high[k][q]);
                }
            } else {
                std::mem::swap(&mut low[k], &mut high[k]);
            }

            if MODE_HAS_SHARED_P_BITS[best_mode as usize] == 0 {
                pbits[k].swap(0, 1);
            }
        }

        if get_bc7_mode_has_seperate_alpha_selectors(best_mode) {
            let alpha_index_bits = get_bc7_alpha_index_size(best_mode, results.index_selector);
            let num_alpha_indices = 1u32 << alpha_index_bits;

            if (alpha_selectors[anchor_index] as u32 & (num_alpha_indices >> 1)) != 0 {
                for i in 0..16 {
                    if partition[i] as usize == k {
                        alpha_selectors[i] = (num_alpha_indices - 1).wrapping_sub(alpha_selectors[i] as u32) as i32;
                    }
                }

                std::mem::swap(&mut low[k][3], &mut high[k][3]);
            }
        }
    }

    *block = [0; BLOCK_SIZE];

    let mut cur_bit_ofs = 0u32;

    set_block_bits(block, 1 << best_mode, best_mode + 1, &mut cur_bit_ofs);

    if best_mode == 4 || best_mode == 5 {
        set_block_bits(block, results.rotation, 2, &mut cur_bit_ofs);
    }

    if best_mode == 4 {
        set_block_bits(block, results.index_selector, 1, &mut cur_bit_ofs);
    }

    if total_partitions > 1 {
        set_block_bits(block, results.partition, if total_partitions == 64 { 6 } else { 4 }, &mut cur_bit_ofs);
    }

    let total_comps = if best_mode >= 4 { 4 } else { 3 };
    for comp in 0..total_comps {
        for subset in 0..total_subsets as usize {
            let bits = if comp == 3 { ALPHA_PRECISION_TABLE[best_mode as usize] } else { COLOR_PRECISION_TABLE[best_mode as usize] };
            set_block_bits(block, low[subset][comp] as u32, bits, &mut cur_bit_ofs);
            set_block_bits(block, high[subset][comp] as u32, bits, &mut cur_bit_ofs);
        }
    }

    if MODE_HAS_P_BITS[best_mode as usize] != 0 {
        for subset in 0..total_subsets as usize {
            set_block_bits(block, pbits[subset][0], 1, &mut cur_bit_ofs);
            if MODE_HAS_SHARED_P_BITS[best_mode as usize] == 0 {
                set_block_bits(block, pbits[subset][1], 1, &mut cur_bit_ofs);
            }
        }
    }

    for y in 0..4 {
        for x in 0..4 {
            let idx = x + y * 4;

            let mut n =
                if results.index_selector != 0 { get_bc7_alpha_index_size(best_mode, results.index_selector) } else { get_bc7_color_index_size(best_mode, results.index_selector) };

            if idx == anchor[0] || idx == anchor[1] || idx == anchor[2] {
                n -= 1;
            }

            set_block_bits(block, if results.index_selector != 0 { alpha_selectors[idx as usize] } else { color_selectors[idx as usize] } as u32, n, &mut cur_bit_ofs);
        }
    }

    if get_bc7_mode_has_seperate_alpha_selectors(best_mode) {
        for y in 0..4 {
            for x in 0..4 {
                let idx = x + y * 4;

                let mut n =
                    if results.index_selector != 0 { get_bc7_color_index_size(best_mode, results.index_selector) } else { get_bc7_alpha_index_size(best_mode, results.index_selector) };

                if idx == anchor[0] || idx == anchor[1] || idx == anchor[2] {
                    n -= 1;
                }

                set_block_bits(block, if results.index_selector != 0 { color_selectors[idx as usize] } else { alpha_selectors[idx as usize] } as u32, n, &mut cur_bit_ofs);
            }
        }
    }

    debug_assert!(cur_bit_ofs == 128);
}

#[inline]
fn encode_bc7_block_mode6(block: &mut [u8; 16], results: &OptimizationResults) {
    let (low, high);
    let mut pbits = [0u32; 2];

    let mut invert_selectors = 0u32;
    if (results.selectors[0] & 8) != 0 {
        invert_selectors = 15;

        low = results.high[0];
        high = results.low[0];

        pbits[0] = results.pbits[0][1];
        pbits[1] = results.pbits[0][0];
    } else {
        low = results.low[0];
        high = results.high[0];

        pbits[0] = results.pbits[0][0];
        pbits[1] = results.pbits[0][1];
    }

    let mut l: u64 = 1 << 6;

    l |= (low[0] as u64) << 7;
    l |= (high[0] as u64) << 14;
    l |= (low[1] as u64) << 21;
    l |= (high[1] as u64) << 28;
    l |= (low[2] as u64) << 35;
    l |= (high[2] as u64) << 42;
    l |= (low[3] as u64) << 49;
    l |= (high[3] as u64) << 56;
    l |= (pbits[0] as u64) << 63;

    let mut h: u64 = pbits[1] as u64;

    h |= ((invert_selectors ^ results.selectors[0] as u32) as u64) << 1;
    for i in 1..16 {
        h |= ((invert_selectors ^ results.selectors[i] as u32) as u64) << (4 * i);
    }

    block[..8].copy_from_slice(&l.to_le_bytes());
    block[8..].copy_from_slice(&h.to_le_bytes());
}

/// Each pixel's alpha matched to the nearest of the first `count` of `vals` (the first of
/// equally near ones): the error, weighted, and the selectors.
#[inline]
fn nearest_alpha_selectors(pixels: &[ColorQuadI; 16], vals: &[i32; 8], count: usize, weight: u32) -> (u64, [i32; 16]) {
    let mut trial_alpha_err = 0u64;
    let mut trial_alpha_selectors = [0i32; 16];

    for i in 0..16 {
        let a = pixels[i][3];

        let mut s = 0;
        let mut be = iabs32(a - vals[0]);

        for k in 1..count {
            let e = iabs32(a - vals[k]);
            if e < be {
                be = e;
                s = k as i32;
            }
        }

        trial_alpha_err += ((be * be) as u32).wrapping_mul(weight) as u64;

        trial_alpha_selectors[i] = s;
    }

    (trial_alpha_err, trial_alpha_selectors)
}

/// Mode 4's alpha values between two 6-bit endpoints: eight when the alpha has the 3-bit
/// selectors (`index_selector` 0), four when it has the 2-bit ones.
#[inline]
fn mode4_alpha_values(la: u32, ha: u32, index_selector: u32) -> ([i32; 8], usize) {
    let mut vals = [0i32; 8];
    if index_selector == 0 {
        vals[0] = ((la << 2) | (la >> 4)) as i32;
        vals[7] = ((ha << 2) | (ha >> 4)) as i32;

        for i in 1..7 {
            vals[i] = (vals[0] * (64 - WEIGHTS3[i] as i32) + vals[7] * WEIGHTS3[i] as i32 + 32) >> 6;
        }
        (vals, 8)
    } else {
        vals[0] = ((la << 2) | (la >> 4)) as i32;
        vals[3] = ((ha << 2) | (ha >> 4)) as i32;

        let (w_s1, w_s2) = (21i32, 43i32);
        vals[1] = (vals[0] * (64 - w_s1) + vals[3] * w_s1 + 32) >> 6;
        vals[2] = (vals[0] * (64 - w_s2) + vals[3] * w_s2 + 32) >> 6;
        (vals, 4)
    }
}

/// Mode 5's four alpha values between two 8-bit endpoints.
#[inline]
fn mode5_alpha_values(lo_a: u32, hi_a: u32) -> [i32; 8] {
    let mut vals = [0i32; 8];
    vals[0] = lo_a as i32;
    vals[3] = hi_a as i32;

    let (w_s1, w_s2) = (21i32, 43i32);
    vals[1] = (vals[0] * (64 - w_s1) + vals[3] * w_s1 + 32) >> 6;
    vals[2] = (vals[0] * (64 - w_s2) + vals[3] * w_s2 + 32) >> 6;
    vals
}

impl Bc7e {
    fn handle_alpha_block_mode4(
        &self,
        pixels: &[ColorQuadI; 16],
        comp_params: &CompressBlockParams,
        params: &mut ColorCellCompressorParams,
        lo_a: u32,
        hi_a: u32,
        opt_results4: &mut OptimizationResults,
        mode4_err: &mut u64,
    ) {
        params.has_alpha = false;
        params.comp_bits = 5;
        params.has_pbits = false;
        params.endpoints_share_pbit = false;
        params.perceptual = comp_params.perceptual;

        for index_selector in 0..2u32 {
            if (comp_params.mode4_index_mask & (1 << index_selector)) == 0 {
                continue;
            }

            if index_selector != 0 {
                params.selector_weights = &WEIGHTS3;
                params.selector_weightsx = &WEIGHTS3X;
                params.num_selector_weights = 8;
            } else {
                params.selector_weights = &WEIGHTS2;
                params.selector_weightsx = &WEIGHTS2X;
                params.num_selector_weights = 4;
            }

            let mut results = ColorCellCompressorResults::new();

            let mut trial_err = self.color_cell_compression(4, params, &mut results, comp_params, pixels, true);
            debug_assert!(trial_err == results.best_overall_err);

            let mut la = (((lo_a + 2) >> 2) as i32).min(63) as u32;
            let mut ha = (((hi_a + 2) >> 2) as i32).min(63) as u32;

            if la == ha {
                if lo_a != hi_a {
                    if ha != 63 {
                        ha += 1;
                    } else if la != 0 {
                        la -= 1;
                    }
                }
            }

            let mut best_alpha_err = u64::MAX;
            let (mut best_la, mut best_ha) = (0u32, 0u32);
            let mut best_alpha_selectors = [0i32; 16];

            for pass in 0..2 {
                let (vals, count) = mode4_alpha_values(la, ha, index_selector);

                let (trial_alpha_err, trial_alpha_selectors) = nearest_alpha_selectors(pixels, &vals, count, params.weights[3]);

                if trial_alpha_err < best_alpha_err {
                    best_alpha_err = trial_alpha_err;
                    best_la = la;
                    best_ha = ha;
                    best_alpha_selectors = trial_alpha_selectors;
                }

                if pass == 0 {
                    let (mut xl, mut xh) = compute_least_squares_endpoints_a(16, &trial_alpha_selectors, if index_selector != 0 { &WEIGHTS2X } else { &WEIGHTS3X }, pixels);
                    if xl > xh {
                        std::mem::swap(&mut xl, &mut xh);
                    }
                    la = clampi(f2i(fmath::floor(xl * (63.0f32 / 255.0f32) + 0.5)), 0, 63) as u32;
                    ha = clampi(f2i(fmath::floor(xh * (63.0f32 / 255.0f32) + 0.5)), 0, 63) as u32;
                }
            }

            if comp_params.uber_level > 0 {
                let d = (comp_params.uber_level as i32).min(3);
                for ld in -d..=d {
                    for hd in -d..=d {
                        la = clampi(best_la as i32 + ld, 0, 63) as u32;
                        ha = clampi(best_ha as i32 + hd, 0, 63) as u32;

                        let (vals, count) = mode4_alpha_values(la, ha, index_selector);

                        let (trial_alpha_err, trial_alpha_selectors) = nearest_alpha_selectors(pixels, &vals, count, params.weights[3]);

                        if trial_alpha_err < best_alpha_err {
                            best_alpha_err = trial_alpha_err;
                            best_la = la;
                            best_ha = ha;
                            best_alpha_selectors = trial_alpha_selectors;
                        }
                    }
                }
            }

            trial_err = trial_err.wrapping_add(best_alpha_err);

            if trial_err < *mode4_err {
                *mode4_err = trial_err;

                opt_results4.mode = 4;
                opt_results4.index_selector = index_selector;
                opt_results4.rotation = 0;
                opt_results4.partition = 0;

                opt_results4.low[0] = results.low_endpoint;
                opt_results4.high[0] = results.high_endpoint;
                opt_results4.low[0][3] = best_la as i32;
                opt_results4.high[0][3] = best_ha as i32;

                opt_results4.selectors = results.selectors;
                opt_results4.alpha_selectors = best_alpha_selectors;
            }
        }
    }

    fn handle_alpha_block_mode5(
        &self,
        pixels: &[ColorQuadI; 16],
        comp_params: &CompressBlockParams,
        params: &mut ColorCellCompressorParams,
        mut lo_a: u32,
        mut hi_a: u32,
        opt_results5: &mut OptimizationResults,
        mode5_err: &mut u64,
    ) {
        params.selector_weights = &WEIGHTS2;
        params.selector_weightsx = &WEIGHTS2X;
        params.num_selector_weights = 4;

        params.comp_bits = 7;
        params.has_alpha = false;
        params.has_pbits = false;
        params.endpoints_share_pbit = false;

        params.perceptual = comp_params.perceptual;

        let mut results5 = ColorCellCompressorResults::new();

        *mode5_err = self.color_cell_compression(5, params, &mut results5, comp_params, pixels, true);
        debug_assert!(*mode5_err == results5.best_overall_err);

        opt_results5.selectors = results5.selectors;
        opt_results5.low[0] = results5.low_endpoint;
        opt_results5.high[0] = results5.high_endpoint;

        if lo_a == hi_a {
            opt_results5.low[0][3] = lo_a as i32;
            opt_results5.high[0][3] = hi_a as i32;
            opt_results5.alpha_selectors = [0; 16];
        } else {
            let mut mode5_alpha_err = u64::MAX;

            for pass in 0..2 {
                let vals = mode5_alpha_values(lo_a, hi_a);

                let (trial_alpha_err, trial_alpha_selectors) = nearest_alpha_selectors(pixels, &vals, 4, params.weights[3]);

                if trial_alpha_err < mode5_alpha_err {
                    mode5_alpha_err = trial_alpha_err;
                    opt_results5.low[0][3] = lo_a as i32;
                    opt_results5.high[0][3] = hi_a as i32;
                    opt_results5.alpha_selectors = trial_alpha_selectors;
                }

                if pass == 0 {
                    let (xl, xh) = compute_least_squares_endpoints_a(16, &trial_alpha_selectors, &WEIGHTS2X, pixels);

                    let mut new_lo_a = clampi(f2i(fmath::floor(xl + 0.5)), 0, 255) as u32;
                    let mut new_hi_a = clampi(f2i(fmath::floor(xh + 0.5)), 0, 255) as u32;
                    if new_lo_a > new_hi_a {
                        std::mem::swap(&mut new_lo_a, &mut new_hi_a);
                    }

                    if new_lo_a == lo_a && new_hi_a == hi_a {
                        break;
                    }

                    lo_a = new_lo_a;
                    hi_a = new_hi_a;
                }
            }

            if comp_params.uber_level > 0 {
                let d = (comp_params.uber_level as i32).min(3);
                for ld in -d..=d {
                    for hd in -d..=d {
                        lo_a = clampi(opt_results5.low[0][3] + ld, 0, 255) as u32;
                        hi_a = clampi(opt_results5.high[0][3] + hd, 0, 255) as u32;

                        let vals = mode5_alpha_values(lo_a, hi_a);

                        let (trial_alpha_err, trial_alpha_selectors) = nearest_alpha_selectors(pixels, &vals, 4, params.weights[3]);

                        if trial_alpha_err < mode5_alpha_err {
                            mode5_alpha_err = trial_alpha_err;
                            opt_results5.low[0][3] = lo_a as i32;
                            opt_results5.high[0][3] = hi_a as i32;
                            opt_results5.alpha_selectors = trial_alpha_selectors;
                        }
                    }
                }
            }

            *mode5_err = mode5_err.wrapping_add(mode5_alpha_err);
        }

        opt_results5.mode = 5;
        opt_results5.index_selector = 0;
        opt_results5.rotation = 0;
        opt_results5.partition = 0;
    }
}

/// The pixels with alpha and one colour channel changed places (`rotation` 1 to 3: red,
/// green, blue; 0: as they are), and the lowest and highest of what is then the alpha.
fn rotate_pixels(pixels: &[ColorQuadI; 16], rotation: u32, lo_a: u32, hi_a: u32) -> ([ColorQuadI; 16], u32, u32) {
    if rotation == 0 {
        return (*pixels, lo_a, hi_a);
    }

    let mut rot_pixels = *pixels;
    let (mut trial_lo_a, mut trial_hi_a) = (255u32, 0u32);
    for c in &mut rot_pixels {
        c.swap(3, rotation as usize - 1);

        trial_lo_a = trial_lo_a.min(c[3] as u32);
        trial_hi_a = trial_hi_a.max(c[3] as u32);
    }
    (rot_pixels, trial_lo_a, trial_hi_a)
}

/// Which of a subset's two p-bits a mode keeps when a partition is stored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StorePbits {
    None,
    First,
    Both,
}

/// A partition's subsets as they were packed, into the block's results.
fn store_partition(opt_results: &mut OptimizationResults, total_subsets: usize, subsets: &Subsets, subset_results: &[ColorCellCompressorResults; 3], pbits: StorePbits) {
    for subset in 0..total_subsets {
        for i in 0..subsets.total_colors[subset] {
            let pixel_index = subsets.pixel_index[subset][i];

            opt_results.selectors[pixel_index] = subset_results[subset].selectors[i];
        }

        opt_results.low[subset] = subset_results[subset].low_endpoint;
        opt_results.high[subset] = subset_results[subset].high_endpoint;

        if pbits != StorePbits::None {
            opt_results.pbits[subset][0] = subset_results[subset].pbits[0];
        }
        if pbits == StorePbits::Both {
            opt_results.pbits[subset][1] = subset_results[subset].pbits[1];
        }
    }
}

impl Bc7e {
    /// A partition's subsets packed one after another, stopping once they are worse together
    /// than the best so far: the error, and what each subset was packed as.
    fn compress_partition(
        &self,
        mode: u32,
        params: &ColorCellCompressorParams,
        comp_params: &CompressBlockParams,
        pixels: &[ColorQuadI; 16],
        partition: u32,
        refinement: bool,
        best_err: u64,
    ) -> (u64, Subsets, [ColorCellCompressorResults; 3]) {
        let total_subsets = NUM_SUBSETS[mode as usize];
        let subsets = Subsets::new(pixels, total_subsets, partition);
        let mut subset_results = [ColorCellCompressorResults::new(); 3];

        let mut trial_err = 0u64;
        for subset in 0..total_subsets as usize {
            let err = self.color_cell_compression(mode, params, &mut subset_results[subset], comp_params, subsets.pixels(subset), refinement);
            debug_assert!(err == subset_results[subset].best_overall_err);

            trial_err = trial_err.wrapping_add(err);
            if trial_err > best_err {
                break;
            }
        }

        (trial_err, subsets, subset_results)
    }

    /// A mode of two or three subsets over the partitions the estimator chose: each tried, the
    /// best kept if it beats what the block has. With more than two partitions to try they
    /// are tried without the refinement passes first (`fast`), and the winner again with them.
    fn try_partitions(
        &self,
        mode: u32,
        params: &ColorCellCompressorParams,
        comp_params: &CompressBlockParams,
        pixels: &[ColorQuadI; 16],
        solutions: &[Solution],
        fast: bool,
        pbits: StorePbits,
        opt_results: &mut OptimizationResults,
        best_err: &mut u64,
    ) {
        let total_subsets = NUM_SUBSETS[mode as usize] as usize;
        let num_solutions = solutions.len();

        for solution in solutions {
            let trial_partition = solution.index;
            debug_assert!(trial_partition < 64);

            let (trial_err, subsets, subset_results) = self.compress_partition(mode, params, comp_params, pixels, trial_partition, !fast || num_solutions <= 2, *best_err);

            if trial_err < *best_err {
                *best_err = trial_err;

                opt_results.mode = mode;
                opt_results.index_selector = 0;
                opt_results.rotation = 0;
                opt_results.partition = trial_partition;

                store_partition(opt_results, total_subsets, &subsets, &subset_results, pbits);
            }
        }

        if fast && num_solutions > 2 && opt_results.mode == mode {
            let trial_partition = opt_results.partition;
            debug_assert!(trial_partition < 64);

            let (trial_err, subsets, subset_results) = self.compress_partition(mode, params, comp_params, pixels, trial_partition, true, *best_err);

            if trial_err < *best_err {
                *best_err = trial_err;

                store_partition(opt_results, total_subsets, &subsets, &subset_results, pbits);
            }
        }
    }

    fn handle_alpha_block(&self, block: &mut [u8; 16], pixels: &[ColorQuadI; 16], comp_params: &CompressBlockParams, params: &mut ColorCellCompressorParams, lo_a: u32, hi_a: u32) {
        params.perceptual = comp_params.perceptual;

        let mut opt_results = OptimizationResults::new();

        let mut best_err = u64::MAX;

        // Mode 4
        if comp_params.alpha_settings.use_mode4 {
            let mut params4 = *params;

            let num_rotations = if comp_params.perceptual || !comp_params.alpha_settings.use_mode4_rotation { 1 } else { 4 };
            for rotation in 0..num_rotations {
                if (comp_params.mode4_rotation_mask & (1 << rotation)) == 0 {
                    continue;
                }

                params4.weights = params.weights;
                if rotation != 0 {
                    params4.weights.swap(rotation as usize - 1, 3);
                }

                let (trial_pixels, trial_lo_a, trial_hi_a) = rotate_pixels(pixels, rotation, lo_a, hi_a);

                let mut trial_opt_results4 = OptimizationResults::new();

                let mut trial_mode4_err = best_err;

                self.handle_alpha_block_mode4(&trial_pixels, comp_params, &mut params4, trial_lo_a, trial_hi_a, &mut trial_opt_results4, &mut trial_mode4_err);

                if trial_mode4_err < best_err {
                    best_err = trial_mode4_err;

                    opt_results.mode = 4;
                    opt_results.index_selector = trial_opt_results4.index_selector;
                    opt_results.rotation = rotation;
                    opt_results.partition = 0;

                    opt_results.low[0] = trial_opt_results4.low[0];
                    opt_results.high[0] = trial_opt_results4.high[0];

                    opt_results.selectors = trial_opt_results4.selectors;
                    opt_results.alpha_selectors = trial_opt_results4.alpha_selectors;
                }
            }
        }

        // Mode 6
        if comp_params.alpha_settings.use_mode6 {
            let mut params6 = *params;

            for c in 0..4 {
                params6.weights[c] = params6.weights[c].wrapping_mul(comp_params.alpha_settings.mode67_error_weight_mul[c]);
            }

            let mut results6 = ColorCellCompressorResults::new();

            params6.selector_weights = &WEIGHTS4;
            params6.selector_weightsx = &WEIGHTS4X;
            params6.num_selector_weights = 16;

            params6.comp_bits = 7;
            params6.has_pbits = true;
            params6.endpoints_share_pbit = false;
            params6.has_alpha = true;

            let mode6_err = self.color_cell_compression(6, &params6, &mut results6, comp_params, pixels, true);
            debug_assert!(mode6_err == results6.best_overall_err);

            if mode6_err < best_err {
                best_err = mode6_err;

                opt_results.mode = 6;
                opt_results.index_selector = 0;
                opt_results.rotation = 0;
                opt_results.partition = 0;

                opt_results.low[0] = results6.low_endpoint;
                opt_results.high[0] = results6.high_endpoint;

                opt_results.pbits[0][0] = results6.pbits[0];
                opt_results.pbits[0][1] = results6.pbits[1];

                opt_results.selectors = results6.selectors;
            }
        }

        // Mode 5
        if comp_params.alpha_settings.use_mode5 {
            let mut params5 = *params;

            let num_rotations = if comp_params.perceptual || !comp_params.alpha_settings.use_mode5_rotation { 1 } else { 4 };
            for rotation in 0..num_rotations {
                if (comp_params.mode5_rotation_mask & (1 << rotation)) == 0 {
                    continue;
                }

                params5.weights = params.weights;
                if rotation != 0 {
                    params5.weights.swap(rotation as usize - 1, 3);
                }

                let (trial_pixels, trial_lo_a, trial_hi_a) = rotate_pixels(pixels, rotation, lo_a, hi_a);

                let mut trial_opt_results5 = OptimizationResults::new();

                let mut trial_mode5_err = 0u64;

                self.handle_alpha_block_mode5(&trial_pixels, comp_params, &mut params5, trial_lo_a, trial_hi_a, &mut trial_opt_results5, &mut trial_mode5_err);

                if trial_mode5_err < best_err {
                    best_err = trial_mode5_err;

                    opt_results = trial_opt_results5;
                    opt_results.rotation = rotation;
                }
            }
        }

        // Mode 7
        if comp_params.alpha_settings.use_mode7 {
            let mut solutions = [Solution::default(); MAX_PARTITIONS7 as usize];
            let num_solutions = estimate_partition_list(7, pixels, comp_params, &mut solutions, comp_params.alpha_settings.max_mode7_partitions_to_try as i32);

            let mut params7 = *params;

            for c in 0..4 {
                params7.weights[c] = params7.weights[c].wrapping_mul(comp_params.alpha_settings.mode67_error_weight_mul[c]);
            }

            params7.selector_weights = &WEIGHTS2;
            params7.selector_weightsx = &WEIGHTS2X;
            params7.num_selector_weights = 4;

            params7.comp_bits = 5;
            params7.has_pbits = true;
            params7.endpoints_share_pbit = false;

            params7.has_alpha = true;

            self.try_partitions(7, &params7, comp_params, pixels, &solutions[..num_solutions as usize], true, StorePbits::Both, &mut opt_results, &mut best_err);
        }

        encode_bc7_block(block, &opt_results);
    }

    fn handle_opaque_block(&self, block: &mut [u8; 16], pixels: &[ColorQuadI; 16], comp_params: &CompressBlockParams, params: &mut ColorCellCompressorParams) {
        let mut opt_results = OptimizationResults::new();

        let mut best_err = u64::MAX;

        // Mode 6
        if comp_params.opaque_settings.use_mode[6] {
            params.selector_weights = &WEIGHTS4;
            params.selector_weightsx = &WEIGHTS4X;
            params.num_selector_weights = 16;

            params.comp_bits = 7;
            params.has_pbits = true;
            params.endpoints_share_pbit = false;
            params.perceptual = comp_params.perceptual;

            let mut results6 = ColorCellCompressorResults::new();

            best_err = self.color_cell_compression(6, params, &mut results6, comp_params, pixels, true);

            opt_results.mode = 6;
            opt_results.index_selector = 0;
            opt_results.rotation = 0;
            opt_results.partition = 0;

            opt_results.selectors = results6.selectors;
            opt_results.low[0] = results6.low_endpoint;
            opt_results.high[0] = results6.high_endpoint;

            opt_results.pbits[0][0] = results6.pbits[0];
            opt_results.pbits[0][1] = results6.pbits[1];
        }

        let mut solutions2 = [Solution::default(); MAX_PARTITIONS3 as usize];
        let mut num_solutions2 = 0u32;

        if comp_params.opaque_settings.use_mode[1] || comp_params.opaque_settings.use_mode[3] {
            if comp_params.opaque_settings.max_mode13_partitions_to_try == 1 {
                solutions2[0].index = estimate_partition(1, pixels, comp_params);
                num_solutions2 = 1;
            } else {
                num_solutions2 = estimate_partition_list(1, pixels, comp_params, &mut solutions2, comp_params.opaque_settings.max_mode13_partitions_to_try as i32);
            }
        }
        let solutions2 = &solutions2[..num_solutions2 as usize];

        // Mode 1
        if comp_params.opaque_settings.use_mode[1] {
            params.selector_weights = &WEIGHTS3;
            params.selector_weightsx = &WEIGHTS3X;
            params.num_selector_weights = 8;

            params.comp_bits = 6;
            params.has_pbits = true;
            params.endpoints_share_pbit = true;
            params.perceptual = comp_params.perceptual;

            self.try_partitions(1, params, comp_params, pixels, solutions2, true, StorePbits::First, &mut opt_results, &mut best_err);
        }

        // Mode 0
        if comp_params.opaque_settings.use_mode[0] {
            let mut solutions3 = [Solution::default(); MAX_PARTITIONS0 as usize];
            let num_solutions3;

            if comp_params.opaque_settings.max_mode0_partitions_to_try == 1 {
                solutions3[0].index = estimate_partition(0, pixels, comp_params);
                num_solutions3 = 1;
            } else {
                num_solutions3 = estimate_partition_list(0, pixels, comp_params, &mut solutions3, comp_params.opaque_settings.max_mode0_partitions_to_try as i32);
            }

            params.selector_weights = &WEIGHTS3;
            params.selector_weightsx = &WEIGHTS3X;
            params.num_selector_weights = 8;

            params.comp_bits = 4;
            params.has_pbits = true;
            params.endpoints_share_pbit = false;
            params.perceptual = comp_params.perceptual;

            self.try_partitions(0, params, comp_params, pixels, &solutions3[..num_solutions3 as usize], false, StorePbits::Both, &mut opt_results, &mut best_err);
        }

        // Mode 3
        if comp_params.opaque_settings.use_mode[3] {
            params.selector_weights = &WEIGHTS2;
            params.selector_weightsx = &WEIGHTS2X;
            params.num_selector_weights = 4;

            params.comp_bits = 7;
            params.has_pbits = true;
            params.endpoints_share_pbit = false;
            params.perceptual = comp_params.perceptual;

            self.try_partitions(3, params, comp_params, pixels, solutions2, true, StorePbits::Both, &mut opt_results, &mut best_err);
        }

        // Mode 5
        if !comp_params.perceptual && comp_params.opaque_settings.use_mode[5] {
            let mut params5 = *params;

            for rotation in 0..4u32 {
                if (comp_params.mode5_rotation_mask & (1 << rotation)) == 0 {
                    continue;
                }

                params5.weights = params.weights;
                if rotation != 0 {
                    params5.weights.swap(rotation as usize - 1, 3);
                }

                let (trial_pixels, trial_lo_a, trial_hi_a) = rotate_pixels(pixels, rotation, 255, 255);

                let mut trial_opt_results5 = OptimizationResults::new();

                let mut trial_mode5_err = 0u64;

                self.handle_alpha_block_mode5(&trial_pixels, comp_params, &mut params5, trial_lo_a, trial_hi_a, &mut trial_opt_results5, &mut trial_mode5_err);

                if trial_mode5_err < best_err {
                    best_err = trial_mode5_err;

                    opt_results = trial_opt_results5;
                    opt_results.rotation = rotation;
                }
            }
        }

        // Mode 2
        if comp_params.opaque_settings.use_mode[2] {
            let mut solutions3 = [Solution::default(); MAX_PARTITIONS2 as usize];
            let num_solutions3;

            if comp_params.opaque_settings.max_mode2_partitions_to_try == 1 {
                solutions3[0].index = estimate_partition(2, pixels, comp_params);
                num_solutions3 = 1;
            } else {
                num_solutions3 = estimate_partition_list(2, pixels, comp_params, &mut solutions3, comp_params.opaque_settings.max_mode2_partitions_to_try as i32);
            }

            params.selector_weights = &WEIGHTS2;
            params.selector_weightsx = &WEIGHTS2X;
            params.num_selector_weights = 4;

            params.comp_bits = 5;
            params.has_pbits = false;
            params.endpoints_share_pbit = false;
            params.perceptual = comp_params.perceptual;

            self.try_partitions(2, params, comp_params, pixels, &solutions3[..num_solutions3 as usize], false, StorePbits::None, &mut opt_results, &mut best_err);
        }

        // Mode 4
        if !comp_params.perceptual && comp_params.opaque_settings.use_mode[4] {
            let mut params4 = *params;

            for rotation in 0..4u32 {
                if (comp_params.mode4_rotation_mask & (1 << rotation)) == 0 {
                    continue;
                }

                params4.weights = params.weights;
                if rotation != 0 {
                    params4.weights.swap(rotation as usize - 1, 3);
                }

                let (trial_pixels, trial_lo_a, trial_hi_a) = rotate_pixels(pixels, rotation, 255, 255);

                let mut trial_opt_results4 = OptimizationResults::new();

                let mut trial_mode4_err = best_err;

                self.handle_alpha_block_mode4(&trial_pixels, comp_params, &mut params4, trial_lo_a, trial_hi_a, &mut trial_opt_results4, &mut trial_mode4_err);

                if trial_mode4_err < best_err {
                    best_err = trial_mode4_err;

                    opt_results.mode = 4;
                    opt_results.index_selector = trial_opt_results4.index_selector;
                    opt_results.rotation = rotation;
                    opt_results.partition = 0;

                    opt_results.low[0] = trial_opt_results4.low[0];
                    opt_results.high[0] = trial_opt_results4.high[0];

                    opt_results.selectors = trial_opt_results4.selectors;
                    opt_results.alpha_selectors = trial_opt_results4.alpha_selectors;
                }
            }
        }

        encode_bc7_block(block, &opt_results);
    }

    // all solid color blocks can be 100% perfectly encoded with just mode 5
    fn handle_block_solid(&self, block: &mut [u8; 16], cr: u32, cg: u32, cb: u32, ca: u32) {
        let er = self.mode_5_optimal_endpoints[cr as usize];
        let eg = self.mode_5_optimal_endpoints[cg as usize];
        let eb = self.mode_5_optimal_endpoints[cb as usize];

        let mut opt = OptimizationResults::new();
        opt.mode = 5;
        opt.low[0] = [(er & 0xFF) as i32, (eg & 0xFF) as i32, (eb & 0xFF) as i32, ca as i32];
        opt.high[0] = [(er >> 8) as i32, (eg >> 8) as i32, (eb >> 8) as i32, ca as i32];
        opt.selectors = [MODE_5_OPTIMAL_INDEX as i32; 16];

        encode_bc7_block(block, &opt);
    }

    fn handle_opaque_block_mode6(&self, block: &mut [u8; 16], pixels: &[ColorQuadI; 16], comp_params: &CompressBlockParams, params: &mut ColorCellCompressorParams) {
        let mut opt_results = OptimizationResults::new();

        // Mode 6
        params.selector_weights = &WEIGHTS4;
        params.selector_weightsx = &WEIGHTS4X;
        params.num_selector_weights = 16;

        params.comp_bits = 7;
        params.has_pbits = true;
        params.endpoints_share_pbit = false;
        params.perceptual = comp_params.perceptual;

        let mut results6 = ColorCellCompressorResults::new();

        self.color_cell_compression(6, params, &mut results6, comp_params, pixels, true);

        opt_results.mode = 6;
        opt_results.selectors = results6.selectors;
        opt_results.low[0] = results6.low_endpoint;
        opt_results.high[0] = results6.high_endpoint;

        opt_results.pbits[0][0] = results6.pbits[0];
        opt_results.pbits[0][1] = results6.pbits[1];

        encode_bc7_block_mode6(block, &opt_results);
    }

    /// Packs one block of 16 RGBA pixels (red first) as a BC7 block.
    pub fn compress_block(&self, block: &mut [u8; 16], pixels: &[ColorRgba; 16], comp_params: &CompressBlockParams) {
        debug_assert!(comp_params.mode4_rotation_mask != 0);
        debug_assert!(comp_params.mode4_index_mask != 0);
        debug_assert!(comp_params.mode5_rotation_mask != 0);
        debug_assert!(comp_params.uber1_mask != 0);

        let mut params = ColorCellCompressorParams::new();
        params.weights = comp_params.weights;

        let mut temp_pixels = [[0i32; 4]; 16];

        let (mut lo_r, mut hi_r) = (255i32, 0i32);
        let (mut lo_g, mut hi_g) = (255i32, 0i32);
        let (mut lo_b, mut hi_b) = (255i32, 0i32);
        let (mut lo_a, mut hi_a) = (255.0f32, 0.0f32);

        for i in 0..16 {
            let c = pixels[i];

            let r = c[0] as i32;
            let g = c[1] as i32;
            let b = c[2] as i32;
            let a = c[3] as i32;

            temp_pixels[i] = [r, g, b, a];

            lo_r = lo_r.min(r);
            hi_r = hi_r.max(r);
            lo_g = lo_g.min(g);
            hi_g = hi_g.max(g);
            lo_b = lo_b.min(b);
            hi_b = hi_b.max(b);

            let fa = a as f32;

            lo_a = minimumf(lo_a, fa);
            hi_a = maximumf(hi_a, fa);
        }

        let all_same = lo_r == hi_r && lo_g == hi_g && lo_b == hi_b && lo_a == hi_a;

        if all_same {
            self.handle_block_solid(block, lo_r as u32, lo_g as u32, lo_b as u32, lo_a as u32);
        } else {
            let has_alpha = lo_a < 255.0;

            // TODO: alpha block mode 6 only
            if has_alpha {
                self.handle_alpha_block(block, &temp_pixels, comp_params, &mut params, f2i(lo_a) as u32, f2i(hi_a) as u32);
            } else if comp_params.mode6_only {
                self.handle_opaque_block_mode6(block, &temp_pixels, comp_params, &mut params);
            } else {
                self.handle_opaque_block(block, &temp_pixels, comp_params, &mut params);
            }
        }
    }

    /// Packs blocks of 16 RGBA pixels each (red first) as BC7 blocks: `bc7e_compress_blocks`.
    /// The work is shared out among all the machine's threads; the blocks are the same however
    /// many there are. [`Bc7e::compress_blocks_on`] takes the number of threads.
    ///
    /// # Panics
    ///
    /// If there are not as many blocks as sets of pixels.
    pub fn compress_blocks(&self, blocks: &mut [[u8; 16]], pixels: &[[ColorRgba; 16]], comp_params: &CompressBlockParams) {
        self.compress_blocks_on(0, blocks, pixels, comp_params);
    }

    /// [`Bc7e::compress_blocks`] on a number of threads: 0 for all the machine has, 1 to stay
    /// on the calling thread.
    ///
    /// # Panics
    ///
    /// If there are not as many blocks as sets of pixels.
    pub fn compress_blocks_on(&self, threads: u32, blocks: &mut [[u8; 16]], pixels: &[[ColorRgba; 16]], comp_params: &CompressBlockParams) {
        assert_eq!(blocks.len(), pixels.len(), "a block for each sixteen pixels");
        parallel::for_each_run(blocks, pixels, threads, |blocks, pixels| {
            for (block, pixels) in blocks.iter_mut().zip(pixels) {
                self.compress_block(block, pixels, comp_params);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bc7decomp::unpack_bc7;

    fn mode_of(block: &[u8; 16]) -> u32 {
        block[0].trailing_zeros()
    }

    fn unpacked(block: &[u8; 16]) -> [ColorRgba; 16] {
        let mut pixels = [[0u8; 4]; 16];
        assert!(unpack_bc7(block, &mut pixels), "mode {}", mode_of(block));
        pixels
    }

    fn worst_difference(a: &[ColorRgba; 16], b: &[ColorRgba; 16]) -> i32 {
        let mut worst = 0;
        for i in 0..16 {
            for c in 0..4 {
                worst = worst.max((a[i][c] as i32 - b[i][c] as i32).abs());
            }
        }
        worst
    }

    const PROFILES: [fn(bool) -> CompressBlockParams; 7] = [
        CompressBlockParams::ultrafast,
        CompressBlockParams::veryfast,
        CompressBlockParams::fast,
        CompressBlockParams::basic,
        CompressBlockParams::slow,
        CompressBlockParams::veryslow,
        CompressBlockParams::slowest,
    ];

    #[test]
    fn the_base_settings_are_the_originals() {
        let params = CompressBlockParams::new(true);
        assert_eq!(params.max_partitions_mode, [16, 64, 64, 64, 0, 0, 0, 64]);
        assert_eq!(params.weights, [128, 64, 16, 256]);
        assert_eq!((params.uber_level, params.refinement_passes, params.uber1_mask), (0, 1, 7));
        assert_eq!((params.mode4_rotation_mask, params.mode4_index_mask, params.mode5_rotation_mask), (15, 3, 15));
        assert!(params.perceptual && !params.pbit_search && !params.mode6_only);
        assert_eq!(params.opaque_settings.use_mode, [true; 7]);
        assert_eq!(CompressBlockParams::new(false).weights, [1, 1, 1, 1]);

        let slowest = CompressBlockParams::slowest(false);
        assert_eq!((slowest.uber_level, slowest.pbit_search, slowest.opaque_settings.max_mode13_partitions_to_try), (4, true, 4));
        assert!(CompressBlockParams::ultrafast(false).mode6_only);
    }

    #[test]
    fn a_block_of_one_colour_is_kept_exactly() {
        // all solid color blocks can be 100% perfectly encoded with just mode 5
        let encoder = Bc7e::new();
        for color in [[0u8, 0, 0, 255], [255, 255, 255, 255], [1, 2, 3, 255], [200, 100, 50, 255], [17, 34, 51, 68], [255, 0, 255, 0], [128, 127, 126, 1]] {
            for profile in PROFILES {
                let mut block = [0u8; 16];
                encoder.compress_block(&mut block, &[color; 16], &profile(false));
                assert_eq!(mode_of(&block), 5);
                assert_eq!(unpacked(&block), [color; 16]);
            }
        }
    }

    #[test]
    fn an_opaque_block_stays_close() {
        let encoder = Bc7e::new();
        let mut source = [[0u8; 4]; 16];
        for (i, pixel) in source.iter_mut().enumerate() {
            *pixel = [(i * 17) as u8, (255 - i * 9) as u8, (40 + i * 3) as u8, 255];
        }
        for profile in PROFILES {
            for perceptual in [false, true] {
                let mut block = [0u8; 16];
                encoder.compress_block(&mut block, &source, &profile(perceptual));
                let pixels = unpacked(&block);
                // (Mode 6 has alpha, and the encoder doesn't count it for an opaque block:
                // its last bit can come out 0, as the original's does.)
                assert!(pixels.iter().all(|pixel| pixel[3] >= 254));
                assert!(worst_difference(&pixels, &source) <= 8, "{}", worst_difference(&pixels, &source));
            }
        }
        // The fastest profile packs opaque blocks with mode 6 only.
        let mut block = [0u8; 16];
        encoder.compress_block(&mut block, &source, &CompressBlockParams::ultrafast(false));
        assert_eq!(mode_of(&block), 6);
    }

    #[test]
    fn a_block_with_alpha_uses_the_modes_that_hold_alpha() {
        let encoder = Bc7e::new();
        let mut source = [[0u8; 4]; 16];
        for (i, pixel) in source.iter_mut().enumerate() {
            *pixel = [(i * 17) as u8, (i * 10) as u8, (255 - i * 12) as u8, (i * 16) as u8];
        }
        for profile in PROFILES {
            let mut block = [0u8; 16];
            encoder.compress_block(&mut block, &source, &profile(false));
            assert!(matches!(mode_of(&block), 4..=7));
            assert!(worst_difference(&unpacked(&block), &source) <= 24, "{}", worst_difference(&unpacked(&block), &source));
        }

        // Each of the four on its own.
        for mode in 4..=7 {
            let mut params = CompressBlockParams::slow(false);
            params.alpha_settings.use_mode4 = mode == 4;
            params.alpha_settings.use_mode5 = mode == 5;
            params.alpha_settings.use_mode6 = mode == 6;
            params.alpha_settings.use_mode7 = mode == 7;
            let mut block = [0u8; 16];
            encoder.compress_block(&mut block, &source, &params);
            assert_eq!(mode_of(&block), mode);
            unpacked(&block);
        }
    }

    #[test]
    fn each_opaque_mode_can_be_had_on_its_own() {
        let encoder = Bc7e::new();
        // Three colours in bands: something for the modes of two and three subsets.
        let mut source = [[0u8; 4]; 16];
        for (i, pixel) in source.iter_mut().enumerate() {
            *pixel = [[250, 10, 10, 255], [10, 250, 10, 255], [10, 10, 250, 255]][(i % 4).min(2)];
        }
        for mode in 0..7 {
            let mut params = CompressBlockParams::slow(false);
            params.opaque_settings.use_mode = [false; 7];
            params.opaque_settings.use_mode[mode] = true;
            let mut block = [0u8; 16];
            encoder.compress_block(&mut block, &source, &params);
            assert_eq!(mode_of(&block) as usize, mode);
            // The modes without alpha unpack opaque; the others at most one step off.
            let lowest = if mode <= 3 { 255 } else { 254 };
            assert!(unpacked(&block).iter().all(|pixel| pixel[3] >= lowest), "mode {mode}");
        }
    }

    #[test]
    fn blocks_are_packed_one_for_each_sixteen_pixels() {
        let encoder = Bc7e::new();
        let params = CompressBlockParams::basic(true);
        let pixels = [[[9u8, 8, 7, 255]; 16], [[1u8, 2, 3, 4]; 16], [[0u8; 4]; 16]];
        let mut together = [[0u8; 16]; 3];
        encoder.compress_blocks(&mut together, &pixels, &params);
        for i in 0..3 {
            let mut alone = [0u8; 16];
            encoder.compress_block(&mut alone, &pixels[i], &params);
            assert_eq!(together[i], alone);
        }

        // Enough of them for more than one thread to take part: the same blocks on all the
        // machine's threads, on one and on three.
        let pixels: Vec<[ColorRgba; 16]> = (0..200u32).map(|i| std::array::from_fn(|k| [(i + k as u32 * 9) as u8, (i * 3) as u8, (k * 16) as u8, if i % 2 == 0 { 255 } else { (k * 17) as u8 }])).collect();
        let params = CompressBlockParams::ultrafast(false);
        let mut on_all = vec![[0u8; 16]; pixels.len()];
        encoder.compress_blocks(&mut on_all, &pixels, &params);
        for threads in [1, 3] {
            let mut blocks = vec![[0u8; 16]; pixels.len()];
            encoder.compress_blocks_on(threads, &mut blocks, &pixels, &params);
            assert_eq!(blocks, on_all, "{threads} threads");
        }
    }

    #[test]
    #[should_panic(expected = "a block for each sixteen pixels")]
    fn the_wrong_number_of_blocks_panics() {
        Bc7e::new().compress_blocks(&mut [[0u8; 16]; 2], &[[[0u8; 4]; 16]; 3], &CompressBlockParams::basic(false));
    }
}
