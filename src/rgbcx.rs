//! BC1 to BC5 block encoding and decoding: the port of `rgbcx.cpp` and `rgbcx.h` (Richard
//! Geldreich, Jr.).
//!
//! The C++ keeps the tables it makes at start-up in globals, filled by `rgbcx::init`. Here they
//! are an [`Rgbcx`], made by [`Rgbcx::new`], and the encoders that need them are its methods.
//! The BC4 and BC5 encoders and all the unpackers need no tables and are plain functions.

use crate::rgbcx_tables::{
    BEST_TOTAL_ORDERINGS3, BEST_TOTAL_ORDERINGS4, MIDPOINT5, MIDPOINT6,
    NUM_UNIQUE_TOTAL_ORDERINGS3, NUM_UNIQUE_TOTAL_ORDERINGS4, UNIQUE_TOTAL_ORDERS3,
    UNIQUE_TOTAL_ORDERS4,
};

/// A pixel: red, green, blue, alpha.
pub type Color32 = [u8; 4];

/// How a GPU works out the two colours between a BC1 block's endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Bc1ApproxMode {
    /// The default mode. No rounding for 4-color colors 2,3. My older tools/compressors use
    /// this mode. This matches the D3D10 docs on BC1.
    #[default]
    Ideal = 0,
    /// NVidia GPU mode.
    NVidia = 1,
    /// AMD GPU mode.
    Amd = 2,
    /// This mode matches AMD Compressonator's output. It rounds 4-color colors 2,3 (not
    /// 3-color color 2). This matches the D3D9 docs on DXT1.
    IdealRound4 = 3,
}

// The flags of `encode_bc1`.

/// Try to improve quality using the most likely total orderings. The total_orderings_to_try
/// parameter will then control the number of total orderings to try for 4 color blocks, and
/// the total_orderings_to_try3 parameter will control the number of total orderings to try for
/// 3 color blocks (if they are enabled).
pub const ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS: u32 = 2;
/// Use 2 least squares pass, instead of one (same as stb_dxt's HIGHQUAL option). Recommended
/// if you're enabling the likely total orderings.
pub const ENCODE_BC1_TWO_LEAST_SQUARES_PASSES: u32 = 4;
/// Enables 3 color mode for blocks containing black or very dark pixels. (Important: the
/// encoder assumes the pixel shader ignores the alpha of a sampled BC1 texture.)
pub const ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS: u32 = 8;
/// If set, try 3-color as well as 4-color blocks on every block.
pub const ENCODE_BC1_USE_3COLOR_BLOCKS: u32 = 16;
/// Enables a second, iterative pass over the likely total orderings.
pub const ENCODE_BC1_ITERATIVE: u32 = 32;
/// A fast first guess at the endpoints from the bounding box (higher quality than the
/// integer one, slower).
pub const ENCODE_BC1_BOUNDING_BOX: u32 = 64;
/// Use a slightly lower quality, but ~30% faster MSE evaluation function for 4-color blocks.
pub const ENCODE_BC1_USE_FASTER_MSE_EVAL: u32 = 128;
/// Examine all colors to compute selectors/MSE (slower than default).
pub const ENCODE_BC1_USE_FULL_MSE_EVAL: u32 = 256;
/// Use 2D least squares+inset+optimal rounding (the method used in Humus's GPU texture
/// encoding demo), instead of PCA.
pub const ENCODE_BC1_USE_2D_LS: u32 = 512;
/// Use 6 power iterations vs. 4 for PCA.
pub const ENCODE_BC1_USE_6_POWER_ITERS: u32 = 2048;
/// Check all total orderings: *very* slow.
pub const ENCODE_BC1_EXHAUSTIVE: u32 = 8192;
/// Try 2 different ways of choosing the initial endpoints.
pub const ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS: u32 = 16384;
/// Same as the bounding box, but implemented using integer math (faster, slightly less
/// quality).
pub const ENCODE_BC1_BOUNDING_BOX_INT: u32 = 32768;
/// Try refining the final endpoints by examining nearby colors: the number of rounds, shifted
/// up by this.
pub const ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_SHIFT: u32 = 22;
pub const ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_MASK: u32 =
    1023 << ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_SHIFT;

pub const MIN_TOTAL_ORDERINGS: u32 = 1;
pub const MAX_TOTAL_ORDERINGS3: u32 = 32;
pub const MAX_TOTAL_ORDERINGS4: u32 = 128;
pub const DEFAULT_TOTAL_ORDERINGS_TO_TRY: u32 = 10;
pub const DEFAULT_TOTAL_ORDERINGS_TO_TRY3: u32 = 1;

/// The lowest and highest level `encode_bc1` knows. Level 0 is fastest, 18 is highest quality.
pub const MIN_LEVEL: u32 = 0;
pub const MAX_LEVEL: u32 = 18;

pub const BC4_DEFAULT_SEARCH_RAD: u32 = 3;
pub const BC4_USE_MODE8_FLAG: u32 = 1;
pub const BC4_USE_MODE6_FLAG: u32 = 2;
pub const BC4_USE_ALL_MODES: u32 = 3;

const TOTAL_ORDER_4_0_16: u32 = 15;
const TOTAL_ORDER_4_1_16: u32 = 700;
const TOTAL_ORDER_4_2_16: u32 = 753;
const TOTAL_ORDER_4_3_16: u32 = 515;

const TOTAL_ORDER_3_0_16: u32 = 12;
const TOTAL_ORDER_3_1_16: u32 = 15;
const TOTAL_ORDER_3_2_16: u32 = 89;

/// A float to an int as C does it on x86 (the instruction the C++ compiles to): toward zero,
/// and the lowest int for what doesn't fit or is not a number.
#[inline]
fn f2i(value: f32) -> i32 {
    if value >= -2_147_483_648.0 && value < 2_147_483_648.0 {
        value as i32
    } else {
        i32::MIN
    }
}

#[inline]
fn iabs(i: i32) -> u32 {
    if i < 0 {
        i.wrapping_neg() as u32
    } else {
        i as u32
    }
}

#[inline]
fn to_5(v: u32) -> u8 {
    let v = v * 31 + 128;
    ((v + (v >> 8)) >> 8) as u8
}

#[inline]
fn to_6(v: u32) -> u8 {
    let v = v * 63 + 128;
    ((v + (v >> 8)) >> 8) as u8
}

#[inline]
fn clampf(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

#[inline]
fn clampi(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

#[inline]
fn squarei(a: i32) -> i32 {
    a.wrapping_mul(a)
}

/// `maximum(a, b, c)` of floats, with the C++'s comparisons.
#[inline]
fn maximum3(a: f32, b: f32, c: f32) -> f32 {
    let ab = if a > b { a } else { b };
    if ab > c {
        ab
    } else {
        c
    }
}

// v0, v1 = unexpanded DXT1 endpoint values (5/6-bits)
// c0, c1 = expanded DXT1 endpoint values (8-bits)
#[inline]
fn interp_5_6_ideal(c0: i32, c1: i32) -> i32 {
    (c0 * 2 + c1) / 3
}
#[inline]
fn interp_5_6_ideal_round(c0: i32, c1: i32) -> i32 {
    (c0 * 2 + c1 + 1) / 3
}
#[inline]
fn interp_half_5_6_ideal(c0: i32, c1: i32) -> i32 {
    (c0 + c1) / 2
}
#[inline]
fn interp_5_nv(v0: i32, v1: i32) -> i32 {
    ((2 * v0 + v1) * 22) / 8
}
#[inline]
fn interp_6_nv(c0: i32, c1: i32) -> i32 {
    let gdiff = c1 - c0;
    (256 * c0 + (gdiff / 4) + 128 + gdiff * 80) / 256
}
#[inline]
fn interp_half_5_nv(v0: i32, v1: i32) -> i32 {
    ((v0 + v1) * 33) / 8
}
#[inline]
fn interp_half_6_nv(c0: i32, c1: i32) -> i32 {
    let gdiff = c1 - c0;
    (256 * c0 + gdiff / 4 + 128 + gdiff * 128) / 256
}
#[inline]
fn interp_5_6_amd(c0: i32, c1: i32) -> i32 {
    (c0 * 43 + c1 * 21 + 32) >> 6
}
#[inline]
fn interp_half_5_6_amd(c0: i32, c1: i32) -> i32 {
    (c0 + c1 + 1) >> 1
}

fn interp_5(v0: i32, v1: i32, c0: i32, c1: i32, mode: Bc1ApproxMode) -> i32 {
    match mode {
        Bc1ApproxMode::NVidia => interp_5_nv(v0, v1),
        Bc1ApproxMode::Amd => interp_5_6_amd(c0, c1),
        Bc1ApproxMode::Ideal => interp_5_6_ideal(c0, c1),
        Bc1ApproxMode::IdealRound4 => interp_5_6_ideal_round(c0, c1),
    }
}

fn interp_6(c0: i32, c1: i32, mode: Bc1ApproxMode) -> i32 {
    match mode {
        Bc1ApproxMode::NVidia => interp_6_nv(c0, c1),
        Bc1ApproxMode::Amd => interp_5_6_amd(c0, c1),
        Bc1ApproxMode::Ideal => interp_5_6_ideal(c0, c1),
        Bc1ApproxMode::IdealRound4 => interp_5_6_ideal_round(c0, c1),
    }
}

fn interp_half_5(v0: i32, v1: i32, c0: i32, c1: i32, mode: Bc1ApproxMode) -> i32 {
    match mode {
        Bc1ApproxMode::NVidia => interp_half_5_nv(v0, v1),
        Bc1ApproxMode::Amd => interp_half_5_6_amd(c0, c1),
        Bc1ApproxMode::Ideal | Bc1ApproxMode::IdealRound4 => interp_half_5_6_ideal(c0, c1),
    }
}

fn interp_half_6(c0: i32, c1: i32, mode: Bc1ApproxMode) -> i32 {
    match mode {
        Bc1ApproxMode::NVidia => interp_half_6_nv(c0, c1),
        Bc1ApproxMode::Amd => interp_half_5_6_amd(c0, c1),
        Bc1ApproxMode::Ideal | Bc1ApproxMode::IdealRound4 => interp_half_5_6_ideal(c0, c1),
    }
}

/// The best pair of endpoint values for one 8-bit value of a solid block, and its error.
#[derive(Clone, Copy, Default)]
struct Bc1MatchEntry {
    hi: u8,
    lo: u8,
    e: u8,
}

fn prepare_bc1_single_color_table(
    expand: &[u8],
    mode: Bc1ApproxMode,
    half: bool,
) -> [Bc1MatchEntry; 256] {
    let size = expand.len() as i32;
    let mut table = [Bc1MatchEntry::default(); 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut lowest_e = 256i32;
        for lo in 0..size {
            let lo_e = i32::from(expand[lo as usize]);
            for hi in 0..size {
                let hi_e = i32::from(expand[hi as usize]);
                let v = match (size == 32, half) {
                    (true, false) => interp_5(hi, lo, hi_e, lo_e, mode),
                    (false, false) => interp_6(hi_e, lo_e, mode),
                    (true, true) => interp_half_5(hi, lo, hi_e, lo_e, mode),
                    (false, true) => interp_half_6(hi_e, lo_e, mode),
                };
                let mut e = iabs(v - i as i32) as i32;

                // We only need to factor in 3% error in BC1 ideal mode.
                if mode == Bc1ApproxMode::Ideal || mode == Bc1ApproxMode::IdealRound4 {
                    e += ((iabs(hi_e - lo_e) * 3) / 100) as i32;
                }

                // Favor equal endpoints, for lower error on actual GPU's which approximate the interpolation.
                if e < lowest_e || (e == lowest_e && lo == hi) {
                    entry.hi = hi as u8;
                    entry.lo = lo as u8;
                    entry.e = e as u8;
                    lowest_e = e;
                }
            }
        }
    }
    table
}

// This table is: 9 * (w * w), 9 * ((1.0f - w) * w), 9 * ((1.0f - w) * (1.0f - w))
// where w is [0,1/3,2/3,1]. 9 is the perfect multiplier.
const WEIGHT_VALS4: [u32; 4] = [0x000009, 0x010204, 0x040201, 0x090000];

// multiplier is 4 for 3-color
const WEIGHT_VALS3: [u32; 3] = [0x000004, 0x040000, 0x010101];

/// `compute_selector_factors4` and `compute_selector_factors3`: (iz00, iz10, iz11) for the
/// weights of a histogram of selectors.
fn compute_selector_factors(weight_accum: u32, numerator: f32) -> [f32; 3] {
    let z00 = ((weight_accum >> 16) & 0xFF) as f32;
    let z10 = ((weight_accum >> 8) & 0xFF) as f32;
    let z11 = (weight_accum & 0xFF) as f32;
    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det.abs() < 1e-8f32 {
        det = 0.0;
    } else {
        det = (numerator / 255.0f32) / det;
    }

    [z11 * det, -z10 * det, z00 * det]
}

#[inline]
fn lookup_total_ordering_index4(hash: &[u16; 4096], hist: &[u8; 4]) -> u32 {
    if hist[0] == 16 {
        TOTAL_ORDER_4_0_16
    } else if hist[1] == 16 {
        TOTAL_ORDER_4_1_16
    } else if hist[2] == 16 {
        TOTAL_ORDER_4_2_16
    } else if hist[3] == 16 {
        TOTAL_ORDER_4_3_16
    } else {
        // Must sum to 16, so m_hist[3] isn't needed.
        u32::from(
            hash[usize::from(hist[0]) | (usize::from(hist[1]) << 4) | (usize::from(hist[2]) << 8)],
        )
    }
}

#[inline]
fn lookup_total_ordering_index3(hash: &[u16; 256], hist: &[u8; 3]) -> u32 {
    if hist[0] == 16 {
        TOTAL_ORDER_3_0_16
    } else if hist[1] == 16 {
        TOTAL_ORDER_3_1_16
    } else if hist[2] == 16 {
        TOTAL_ORDER_3_2_16
    } else {
        // Must sum to 16, so m_hist[2] isn't needed.
        u32::from(hash[usize::from(hist[0]) | (usize::from(hist[1]) << 4)])
    }
}

/// The encoder for BC1 and BC3 blocks: the tables `rgbcx::init` makes, for one way of
/// interpolating.
#[derive(Clone)]
pub struct Rgbcx {
    approx_mode: Bc1ApproxMode,
    match5_equals_1: [Bc1MatchEntry; 256],
    match6_equals_1: [Bc1MatchEntry; 256],
    match5_half: [Bc1MatchEntry; 256],
    match6_half: [Bc1MatchEntry; 256],
    total_ordering4_hash: Box<[u16; 4096]>,
    selector_factors4: Vec<[f32; 3]>,
    total_ordering3_hash: [u16; 256],
    selector_factors3: Vec<[f32; 3]>,
}

impl Default for Rgbcx {
    fn default() -> Rgbcx {
        Rgbcx::new(Bc1ApproxMode::Ideal)
    }
}

/// The results of the search for a BC1 block's endpoints and selectors.
#[derive(Clone, Copy, Default)]
struct Bc1EncodeResults {
    lr: i32,
    lg: i32,
    lb: i32,
    hr: i32,
    hg: i32,
    hb: i32,
    sels: [u8; 16],
    is_3color: bool,
}

type Vec3F = [f32; 3];

impl Rgbcx {
    /// `rgbcx::init`: the encoder with its tables. `mode` is how the blocks will be decoded.
    /// The tables of a mode are made the first time and copied from then on.
    pub fn new(mode: Bc1ApproxMode) -> Rgbcx {
        static TABLES: [std::sync::OnceLock<Rgbcx>; 4] = [const { std::sync::OnceLock::new() }; 4];
        TABLES[mode as usize]
            .get_or_init(|| Rgbcx::make_tables(mode))
            .clone()
    }

    /// Makes the tables.
    fn make_tables(mode: Bc1ApproxMode) -> Rgbcx {
        let mut bc1_expand5 = [0u8; 32];
        for (i, value) in bc1_expand5.iter_mut().enumerate() {
            *value = ((i << 3) | (i >> 2)) as u8;
        }
        let mut bc1_expand6 = [0u8; 64];
        for (i, value) in bc1_expand6.iter_mut().enumerate() {
            *value = ((i << 2) | (i >> 4)) as u8;
        }

        let mut total_ordering4_hash = Box::new([0u16; 4096]);
        let mut selector_factors4 = Vec::with_capacity(NUM_UNIQUE_TOTAL_ORDERINGS4);
        for (i, hist) in UNIQUE_TOTAL_ORDERS4.iter().enumerate() {
            if !hist.contains(&16) {
                let index = usize::from(hist[0])
                    | (usize::from(hist[1]) << 4)
                    | (usize::from(hist[2]) << 8);
                total_ordering4_hash[index] = i as u16;
            }
            let mut weight_accum = 0u32;
            for sel in 0..4 {
                weight_accum += WEIGHT_VALS4[sel] * u32::from(hist[sel]);
            }
            selector_factors4.push(compute_selector_factors(weight_accum, 3.0));
        }

        let mut total_ordering3_hash = [0u16; 256];
        let mut selector_factors3 = Vec::with_capacity(NUM_UNIQUE_TOTAL_ORDERINGS3);
        for (i, hist) in UNIQUE_TOTAL_ORDERS3.iter().enumerate() {
            if !hist.contains(&16) {
                let index = usize::from(hist[0]) | (usize::from(hist[1]) << 4);
                total_ordering3_hash[index] = i as u16;
            }
            let mut weight_accum = 0u32;
            for sel in 0..3 {
                weight_accum += WEIGHT_VALS3[sel] * u32::from(hist[sel]);
            }
            selector_factors3.push(compute_selector_factors(weight_accum, 2.0));
        }

        Rgbcx {
            approx_mode: mode,
            match5_equals_1: prepare_bc1_single_color_table(&bc1_expand5, mode, false),
            match6_equals_1: prepare_bc1_single_color_table(&bc1_expand6, mode, false),
            match5_half: prepare_bc1_single_color_table(&bc1_expand5, mode, true),
            match6_half: prepare_bc1_single_color_table(&bc1_expand6, mode, true),
            total_ordering4_hash,
            selector_factors4,
            total_ordering3_hash,
            selector_factors3,
        }
    }

    /// The way of interpolating this encoder was made for.
    pub fn approx_mode(&self) -> Bc1ApproxMode {
        self.approx_mode
    }

    /// Optimally encodes a solid colour block to BC1 format.
    pub fn encode_bc1_solid_block(
        &self,
        dst: &mut [u8; 8],
        fr: u32,
        fg: u32,
        fb: u32,
        allow_3color: bool,
    ) {
        let (fr, fg, fb) = (fr as usize & 255, fg as usize & 255, fb as usize & 255);
        let mut mask = 0xAAu32;
        let (mut max16, mut min16) = (-1i32, 0i32);

        if allow_3color {
            let err4 = u32::from(self.match5_equals_1[fr].e)
                + u32::from(self.match6_equals_1[fg].e)
                + u32::from(self.match5_equals_1[fb].e);
            let err3 = u32::from(self.match5_half[fr].e)
                + u32::from(self.match6_half[fg].e)
                + u32::from(self.match5_half[fb].e);

            if err3 < err4 {
                max16 = (i32::from(self.match5_half[fr].hi) << 11)
                    | (i32::from(self.match6_half[fg].hi) << 5)
                    | i32::from(self.match5_half[fb].hi);
                min16 = (i32::from(self.match5_half[fr].lo) << 11)
                    | (i32::from(self.match6_half[fg].lo) << 5)
                    | i32::from(self.match5_half[fb].lo);

                if max16 > min16 {
                    std::mem::swap(&mut max16, &mut min16);
                }
            }
        }

        if max16 == -1 {
            max16 = (i32::from(self.match5_equals_1[fr].hi) << 11)
                | (i32::from(self.match6_equals_1[fg].hi) << 5)
                | i32::from(self.match5_equals_1[fb].hi);
            min16 = (i32::from(self.match5_equals_1[fr].lo) << 11)
                | (i32::from(self.match6_equals_1[fg].lo) << 5)
                | i32::from(self.match5_equals_1[fb].lo);

            if min16 == max16 {
                // Always forbid 3 color blocks
                // This is to guarantee that BC3 blocks never use punchthrough alpha (3 color) mode, which isn't supported on some (all?) GPU's.
                mask = 0;

                // Make l > h
                if min16 > 0 {
                    min16 -= 1;
                } else {
                    // l = h = 0
                    max16 = 1;
                    min16 = 0;
                    mask = 0x55;
                }
            }

            if max16 < min16 {
                std::mem::swap(&mut max16, &mut min16);
                mask ^= 0x55;
            }
        }

        dst[..2].copy_from_slice(&(max16 as u16).to_le_bytes());
        dst[2..4].copy_from_slice(&(min16 as u16).to_le_bytes());
        dst[4..].fill(mask as u8);
    }
}

fn compute_least_squares_endpoints4_rgb_sums(
    xl: &mut Vec3F,
    xh: &mut Vec3F,
    total: [i32; 3],
    iz: [f32; 3],
    s: usize,
    sums: &[[u32; 17]; 3],
) {
    let (iz00, iz10, iz11) = (iz[0], iz[1], iz[2]);
    let iz01 = iz10;

    let order = &UNIQUE_TOTAL_ORDERS4[s];
    let f1 = usize::from(order[0]);
    let f2 = f1 + usize::from(order[1]);
    let f3 = f2 + usize::from(order[2]);
    for c in 0..3 {
        let sum = &sums[c];
        let uq00 = (sum[f2].wrapping_sub(sum[f1]))
            .wrapping_add((sum[f3].wrapping_sub(sum[f2])).wrapping_mul(2))
            .wrapping_add((sum[16].wrapping_sub(sum[f3])).wrapping_mul(3));
        let q10 = ((total[c].wrapping_mul(3) as u32).wrapping_sub(uq00)) as f32;
        xl[c] = iz00 * uq00 as f32 + iz01 * q10;
        xh[c] = iz10 * uq00 as f32 + iz11 * q10;
    }
}

fn compute_least_squares_endpoints4_rgb(
    colors: &[Color32; 16],
    selectors: &[u8; 16],
    xl: &mut Vec3F,
    xh: &mut Vec3F,
    total: [i32; 3],
) -> bool {
    let mut uq00 = [0u32; 3];
    let mut weight_accum = 0u32;
    for i in 0..16 {
        let sel = u32::from(selectors[i]);
        weight_accum += WEIGHT_VALS4[sel as usize & 3];
        for c in 0..3 {
            uq00[c] += sel * u32::from(colors[i][c]);
        }
    }

    let z00 = ((weight_accum >> 16) & 0xFF) as f32;
    let z10 = ((weight_accum >> 8) & 0xFF) as f32;
    let z11 = (weight_accum & 0xFF) as f32;
    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det.abs() < 1e-8f32 {
        return false;
    }

    det = (3.0f32 / 255.0f32) / det;

    let iz00 = z11 * det;
    let iz01 = -z01 * det;
    let iz10 = -z10 * det;
    let iz11 = z00 * det;

    for c in 0..3 {
        let q10 = (total[c].wrapping_mul(3) as u32).wrapping_sub(uq00[c]) as i32;
        xl[c] = iz00 * uq00[c] as f32 + iz01 * q10 as f32;
        xh[c] = iz10 * uq00[c] as f32 + iz11 * q10 as f32;
    }
    true
}

fn compute_least_squares_endpoints3_rgb_sums(
    xl: &mut Vec3F,
    xh: &mut Vec3F,
    total: [i32; 3],
    iz: [f32; 3],
    s: usize,
    sums: &[[u32; 17]; 3],
) {
    let (iz00, iz10, iz11) = (iz[0], iz[1], iz[2]);
    let iz01 = iz10;

    // Compensates for BC1 3-color ordering, which is selector 0, 2, 1
    let order = &UNIQUE_TOTAL_ORDERS3[s];
    let f1 = usize::from(order[0]);
    let f2 = f1 + usize::from(order[2]);
    for c in 0..3 {
        let sum = &sums[c];
        let uq00 = (sum[16].wrapping_sub(sum[f2]))
            .wrapping_mul(2)
            .wrapping_add(sum[f2].wrapping_sub(sum[f1]));
        let q10 = ((total[c].wrapping_mul(2) as u32).wrapping_sub(uq00)) as f32;
        xl[c] = iz00 * uq00 as f32 + iz01 * q10;
        xh[c] = iz10 * uq00 as f32 + iz11 * q10;
    }
}

fn compute_least_squares_endpoints3_rgb(
    use_black: bool,
    colors: &[Color32; 16],
    selectors: &[u8; 16],
    xl: &mut Vec3F,
    xh: &mut Vec3F,
) -> bool {
    let mut uq00 = [0i32; 3];
    let mut weight_accum = 0u32;
    let mut total = [0i32; 3];
    for i in 0..16 {
        let color = colors[i];
        if use_black && (color[0] | color[1] | color[2]) < 4 {
            continue;
        }

        let sel = selectors[i];
        if sel == 3 {
            continue;
        }

        weight_accum += WEIGHT_VALS3[usize::from(sel) % 3];

        const TRAN: [i32; 3] = [0, 2, 1];
        let tsel = TRAN[usize::from(sel) % 3];
        for c in 0..3 {
            uq00[c] += tsel * i32::from(color[c]);
            total[c] += i32::from(color[c]);
        }
    }

    let z00 = ((weight_accum >> 16) & 0xFF) as f32;
    let z10 = ((weight_accum >> 8) & 0xFF) as f32;
    let z11 = (weight_accum & 0xFF) as f32;
    let z01 = z10;

    let mut det = z00 * z11 - z01 * z10;
    if det.abs() < 1e-8f32 {
        return false;
    }

    det = (2.0f32 / 255.0f32) / det;

    let iz00 = z11 * det;
    let iz01 = -z01 * det;
    let iz10 = -z10 * det;
    let iz11 = z00 * det;

    for c in 0..3 {
        let q10 = total[c] * 2 - uq00[c];
        xl[c] = iz00 * uq00[c] as f32 + iz01 * q10 as f32;
        xh[c] = iz10 * uq00[c] as f32 + iz11 * q10 as f32;
    }
    true
}

fn precise_round_565(xl: &Vec3F, xh: &Vec3F) -> [i32; 6] {
    let mut trial_lr = f2i(xl[0] * 31.0f32);
    let mut trial_lg = f2i(xl[1] * 63.0f32);
    let mut trial_lb = f2i(xl[2] * 31.0f32);
    let mut trial_hr = f2i(xh[0] * 31.0f32);
    let mut trial_hg = f2i(xh[1] * 63.0f32);
    let mut trial_hb = f2i(xh[2] * 31.0f32);

    if (trial_lr | trial_lb | trial_hr | trial_hb) as u32 > 31 {
        let fix = |v: i32| if v as u32 > 31 { (!v >> 31) & 31 } else { v };
        trial_lr = fix(trial_lr);
        trial_hr = fix(trial_hr);
        trial_lb = fix(trial_lb);
        trial_hb = fix(trial_hb);
    }

    if (trial_lg | trial_hg) as u32 > 63 {
        let fix = |v: i32| if v as u32 > 63 { (!v >> 31) & 63 } else { v };
        trial_lg = fix(trial_lg);
        trial_hg = fix(trial_hg);
    }

    trial_lr = (trial_lr + i32::from(xl[0] > MIDPOINT5[trial_lr as usize])) & 31;
    trial_lg = (trial_lg + i32::from(xl[1] > MIDPOINT6[trial_lg as usize])) & 63;
    trial_lb = (trial_lb + i32::from(xl[2] > MIDPOINT5[trial_lb as usize])) & 31;

    trial_hr = (trial_hr + i32::from(xh[0] > MIDPOINT5[trial_hr as usize])) & 31;
    trial_hg = (trial_hg + i32::from(xh[1] > MIDPOINT6[trial_hg as usize])) & 63;
    trial_hb = (trial_hb + i32::from(xh[2] > MIDPOINT5[trial_hb as usize])) & 31;

    [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb]
}

fn precise_round_565_noscale(mut xl: Vec3F, mut xh: Vec3F) -> [i32; 6] {
    for c in 0..3 {
        xl[c] *= 1.0f32 / 255.0f32;
    }
    for c in 0..3 {
        xh[c] *= 1.0f32 / 255.0f32;
    }
    precise_round_565(&xl, &xh)
}

#[inline]
fn pack_unscaled_color(r: i32, g: i32, b: i32) -> u32 {
    u32::from((b as u32 | ((g as u32) << 5) | ((r as u32) << 11)) as u16)
}

fn bc1_encode4(
    dst: &mut [u8; 8],
    lr: i32,
    lg: i32,
    lb: i32,
    hr: i32,
    hg: i32,
    hb: i32,
    sels: &[u8; 16],
) {
    let mut lc16 = pack_unscaled_color(lr, lg, lb);
    let mut hc16 = pack_unscaled_color(hr, hg, hb);

    // Always forbid 3 color blocks
    if lc16 == hc16 {
        let mut mask = 0u8;

        // Make l > h
        if hc16 > 0 {
            hc16 -= 1;
        } else {
            // lc16 = hc16 = 0
            hc16 = 0;
            lc16 = 1;
            mask = 0x55; // select hc16
        }

        dst[..2].copy_from_slice(&(lc16 as u16).to_le_bytes());
        dst[2..4].copy_from_slice(&(hc16 as u16).to_le_bytes());
        dst[4..].fill(mask);
    } else {
        let mut invert_mask = 0u8;
        if lc16 < hc16 {
            std::mem::swap(&mut lc16, &mut hc16);
            invert_mask = 0x55;
        }

        dst[..2].copy_from_slice(&(lc16 as u16).to_le_bytes());
        dst[2..4].copy_from_slice(&(hc16 as u16).to_le_bytes());

        let mut packed_sels = 0u32;
        const SEL_TRANS: [u32; 4] = [0, 2, 3, 1];
        for i in 0..16 {
            packed_sels |= SEL_TRANS[usize::from(sels[i]) & 3] << (i * 2);
        }

        dst[4] = packed_sels as u8 ^ invert_mask;
        dst[5] = (packed_sels >> 8) as u8 ^ invert_mask;
        dst[6] = (packed_sels >> 16) as u8 ^ invert_mask;
        dst[7] = (packed_sels >> 24) as u8 ^ invert_mask;
    }
}

fn bc1_encode3(
    dst: &mut [u8; 8],
    lr: i32,
    lg: i32,
    lb: i32,
    hr: i32,
    hg: i32,
    hb: i32,
    sels: &[u8; 16],
) {
    let mut lc16 = pack_unscaled_color(lr, lg, lb);
    let mut hc16 = pack_unscaled_color(hr, hg, hb);

    let mut invert_flag = false;
    if lc16 > hc16 {
        std::mem::swap(&mut lc16, &mut hc16);
        invert_flag = true;
    }

    dst[..2].copy_from_slice(&(lc16 as u16).to_le_bytes());
    dst[2..4].copy_from_slice(&(hc16 as u16).to_le_bytes());

    let mut packed_sels = 0u32;
    if invert_flag {
        const SEL_TRANS_INV: [u32; 4] = [1, 0, 2, 3];
        for i in 0..16 {
            packed_sels |= SEL_TRANS_INV[usize::from(sels[i]) & 3] << (i * 2);
        }
    } else {
        for i in 0..16 {
            packed_sels |= u32::from(sels[i]) << (i * 2);
        }
    }

    dst[4..].copy_from_slice(&packed_sels.to_le_bytes());
}

/// The pixels sorted along the line between two endpoints, as sums of each colour over the
/// pixels before each place: `r_sum`, `g_sum` and `b_sum`, with the totals at [16].
fn sums_along_axis(
    pixels: &[Color32; 16],
    lr: i32,
    lg: i32,
    lb: i32,
    hr: i32,
    hg: i32,
    hb: i32,
    total: [i32; 3],
) -> [[u32; 17]; 3] {
    let r0 = (lr << 3) | (lr >> 2);
    let g0 = (lg << 2) | (lg >> 4);
    let b0 = (lb << 3) | (lb >> 2);
    let r3 = (hr << 3) | (hr >> 2);
    let g3 = (hg << 2) | (hg >> 4);
    let b3 = (hb << 3) | (hb >> 2);

    let (ar, ag, ab) = (r3 - r0, g3 - g0, b3 - b0);

    let mut dots = [0i32; 16];
    for i in 0..16 {
        let (r, g, b) = (
            i32::from(pixels[i][0]),
            i32::from(pixels[i][1]),
            i32::from(pixels[i][2]),
        );
        let d = 0x1000000 + (r * ar + g * ag + b * ab);
        dots[i] = (d << 4) + i as i32;
    }

    dots.sort_unstable();

    let mut sums = [[0u32; 17]; 3];
    let mut running = [0u32; 3];
    for i in 0..16 {
        let p = (dots[i] & 15) as usize;
        for c in 0..3 {
            sums[c][i] = running[c];
            running[c] += u32::from(pixels[p][c]);
        }
    }
    for c in 0..3 {
        sums[c][16] = total[c] as u32;
    }
    sums
}

impl Rgbcx {
    fn bc1_get_block_colors4(
        &self,
        lr: u32,
        lg: u32,
        lb: u32,
        hr: u32,
        hg: u32,
        hb: u32,
    ) -> [[u32; 4]; 3] {
        let mut block_r = [0u32; 4];
        let mut block_g = [0u32; 4];
        let mut block_b = [0u32; 4];
        block_r[0] = (lr << 3) | (lr >> 2);
        block_g[0] = (lg << 2) | (lg >> 4);
        block_b[0] = (lb << 3) | (lb >> 2);
        block_r[3] = (hr << 3) | (hr >> 2);
        block_g[3] = (hg << 2) | (hg >> 4);
        block_b[3] = (hb << 3) | (hb >> 2);

        match self.approx_mode {
            Bc1ApproxMode::Ideal => {
                for block in [&mut block_r, &mut block_g, &mut block_b] {
                    block[1] = (block[0] * 2 + block[3]) / 3;
                    block[2] = (block[3] * 2 + block[0]) / 3;
                }
            }
            Bc1ApproxMode::IdealRound4 => {
                for block in [&mut block_r, &mut block_g, &mut block_b] {
                    block[1] = (block[0] * 2 + block[3] + 1) / 3;
                    block[2] = (block[3] * 2 + block[0] + 1) / 3;
                }
            }
            Bc1ApproxMode::Amd => {
                for block in [&mut block_r, &mut block_g, &mut block_b] {
                    block[1] = interp_5_6_amd(block[0] as i32, block[3] as i32) as u32;
                    block[2] = interp_5_6_amd(block[3] as i32, block[0] as i32) as u32;
                }
            }
            Bc1ApproxMode::NVidia => {
                block_r[1] = interp_5_nv(lr as i32, hr as i32) as u32;
                block_g[1] = interp_6_nv(block_g[0] as i32, block_g[3] as i32) as u32;
                block_b[1] = interp_5_nv(lb as i32, hb as i32) as u32;
                block_r[2] = interp_5_nv(hr as i32, lr as i32) as u32;
                block_g[2] = interp_6_nv(block_g[3] as i32, block_g[0] as i32) as u32;
                block_b[2] = interp_5_nv(hb as i32, lb as i32) as u32;
            }
        }
        [block_r, block_g, block_b]
    }

    /// The three colours of a 3-colour block; the fourth of each row is zero (black).
    fn bc1_get_block_colors3(
        &self,
        lr: u32,
        lg: u32,
        lb: u32,
        hr: u32,
        hg: u32,
        hb: u32,
    ) -> [[u32; 4]; 3] {
        let mut block_r = [0u32; 4];
        let mut block_g = [0u32; 4];
        let mut block_b = [0u32; 4];
        block_r[0] = (lr << 3) | (lr >> 2);
        block_g[0] = (lg << 2) | (lg >> 4);
        block_b[0] = (lb << 3) | (lb >> 2);
        block_r[1] = (hr << 3) | (hr >> 2);
        block_g[1] = (hg << 2) | (hg >> 4);
        block_b[1] = (hb << 3) | (hb >> 2);

        match self.approx_mode {
            Bc1ApproxMode::Ideal | Bc1ApproxMode::IdealRound4 => {
                block_r[2] = (block_r[0] + block_r[1]) / 2;
                block_g[2] = (block_g[0] + block_g[1]) / 2;
                block_b[2] = (block_b[0] + block_b[1]) / 2;
            }
            Bc1ApproxMode::Amd => {
                block_r[2] = interp_half_5_6_amd(block_r[0] as i32, block_r[1] as i32) as u32;
                block_g[2] = interp_half_5_6_amd(block_g[0] as i32, block_g[1] as i32) as u32;
                block_b[2] = interp_half_5_6_amd(block_b[0] as i32, block_b[1] as i32) as u32;
            }
            Bc1ApproxMode::NVidia => {
                block_r[2] = interp_half_5_nv(lr as i32, hr as i32) as u32;
                block_g[2] = interp_half_6_nv(block_g[0] as i32, block_g[1] as i32) as u32;
                block_b[2] = interp_half_5_nv(lb as i32, hb as i32) as u32;
            }
        }
        [block_r, block_g, block_b]
    }

    fn bc1_find_sels4_noerr(
        &self,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        force_selectors: Option<&[u8; 16]>,
    ) {
        if let Some(forced) = force_selectors {
            *sels = *forced;
            return;
        }

        let [block_r, block_g, block_b] = self.bc1_get_block_colors4(
            lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
        );

        let mut ar = block_r[3] as i32 - block_r[0] as i32;
        let mut ag = block_g[3] as i32 - block_g[0] as i32;
        let mut ab = block_b[3] as i32 - block_b[0] as i32;

        let mut dots = [0i32; 4];
        for i in 0..4 {
            dots[i] = block_r[i] as i32 * ar + block_g[i] as i32 * ag + block_b[i] as i32 * ab;
        }

        let (t0, t1, t2) = (dots[0] + dots[1], dots[1] + dots[2], dots[2] + dots[3]);

        ar *= 2;
        ag *= 2;
        ab *= 2;

        const SELS: [u8; 4] = [3, 2, 1, 0];

        for i in 0..16 {
            let d = i32::from(pixels[i][0]) * ar
                + i32::from(pixels[i][1]) * ag
                + i32::from(pixels[i][2]) * ab;
            sels[i] = SELS[usize::from(d <= t0) + usize::from(d < t1) + usize::from(d < t2)];
        }
    }

    fn bc1_find_sels4_fasterr(
        &self,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        cur_err: u32,
    ) -> u32 {
        let [block_r, block_g, block_b] = self.bc1_get_block_colors4(
            lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
        );

        let mut ar = block_r[3] as i32 - block_r[0] as i32;
        let mut ag = block_g[3] as i32 - block_g[0] as i32;
        let mut ab = block_b[3] as i32 - block_b[0] as i32;

        let mut dots = [0i32; 4];
        for i in 0..4 {
            dots[i] = block_r[i] as i32 * ar + block_g[i] as i32 * ag + block_b[i] as i32 * ab;
        }

        let (t0, t1, t2) = (dots[0] + dots[1], dots[1] + dots[2], dots[2] + dots[3]);

        ar *= 2;
        ag *= 2;
        ab *= 2;

        const SELS: [u8; 4] = [3, 2, 1, 0];

        let mut total_err = 0u32;

        for row in 0..4 {
            for i in row * 4..row * 4 + 4 {
                let d = i32::from(pixels[i][0]) * ar
                    + i32::from(pixels[i][1]) * ag
                    + i32::from(pixels[i][2]) * ab;
                sels[i] = SELS[usize::from(d <= t0) + usize::from(d < t1) + usize::from(d < t2)];
            }
            for i in row * 4..row * 4 + 4 {
                let sel = usize::from(sels[i]);
                let err = squarei(i32::from(pixels[i][0]) - block_r[sel] as i32)
                    + squarei(i32::from(pixels[i][1]) - block_g[sel] as i32)
                    + squarei(i32::from(pixels[i][2]) - block_b[sel] as i32);
                total_err = total_err.wrapping_add(err as u32);
            }

            if total_err >= cur_err {
                break;
            }
        }

        total_err
    }

    fn bc1_find_sels4_check2_err(
        &self,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        cur_err: u32,
    ) -> u32 {
        let [block_r, block_g, block_b] = self.bc1_get_block_colors4(
            lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
        );
        let block = |channel: &[u32; 4], sel: i32| channel[sel as usize] as i32;

        let dr = block_r[3] as i32 - block_r[0] as i32;
        let dg = block_g[3] as i32 - block_g[0] as i32;
        let db = block_b[3] as i32 - block_b[0] as i32;

        let f = 4.0f32 / ((squarei(dr) + squarei(dg) + squarei(db)) as f32 + 0.00000125f32);

        let mut total_err = 0u32;

        for i in 0..16 {
            let (r, g, b) = (
                i32::from(pixels[i][0]),
                i32::from(pixels[i][1]),
                i32::from(pixels[i][2]),
            );

            let mut sel = f2i(((r - block_r[0] as i32) * dr
                + (g - block_g[0] as i32) * dg
                + (b - block_b[0] as i32) * db) as f32
                * f
                + 0.5f32);
            sel = clampi(sel, 1, 3);

            let err0 = (squarei(block(&block_r, sel - 1) - r)
                + squarei(block(&block_g, sel - 1) - g)
                + squarei(block(&block_b, sel - 1) - b)) as u32;
            let err1 = (squarei(block(&block_r, sel) - r)
                + squarei(block(&block_g, sel) - g)
                + squarei(block(&block_b, sel) - b)) as u32;

            let mut best_sel = sel;
            let mut best_err = err1;
            if err0 == err1 {
                // Prefer non-interpolation
                if best_sel - 1 == 0 {
                    best_sel = 0;
                }
            } else if err0 < best_err {
                best_sel = sel - 1;
                best_err = err0;
            }

            total_err = total_err.wrapping_add(best_err);

            if total_err >= cur_err {
                break;
            }

            sels[i] = best_sel as u8;
        }
        total_err
    }

    fn bc1_find_sels4_fullerr(
        &self,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        cur_err: u32,
    ) -> u32 {
        let [block_r, block_g, block_b] = self.bc1_get_block_colors4(
            lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
        );

        let mut total_err = 0u32;

        for i in 0..16 {
            let (r, g, b) = (
                i32::from(pixels[i][0]),
                i32::from(pixels[i][1]),
                i32::from(pixels[i][2]),
            );

            let mut best_err = (squarei(block_r[0] as i32 - r)
                + squarei(block_g[0] as i32 - g)
                + squarei(block_b[0] as i32 - b)) as u32;
            let mut best_sel = 0u8;

            let mut j = 1;
            while j < 4 && best_err != 0 {
                let err = (squarei(block_r[j] as i32 - r)
                    + squarei(block_g[j] as i32 - g)
                    + squarei(block_b[j] as i32 - b)) as u32;
                if err < best_err || (err == best_err && j == 3) {
                    best_err = err;
                    best_sel = j as u8;
                }
                j += 1;
            }

            total_err = total_err.wrapping_add(best_err);

            if total_err >= cur_err {
                break;
            }

            sels[i] = best_sel;
        }
        total_err
    }

    fn bc1_find_sels4(
        &self,
        flags: u32,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        cur_err: u32,
        force_selectors: Option<&[u8; 16]>,
    ) -> u32 {
        if let Some(forced) = force_selectors {
            *sels = *forced;

            let [block_r, block_g, block_b] = self.bc1_get_block_colors4(
                lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
            );

            let mut err = 0u32;
            for i in 0..16 {
                let (r, g, b) = (
                    i32::from(pixels[i][0]),
                    i32::from(pixels[i][1]),
                    i32::from(pixels[i][2]),
                );
                let sel = usize::from(forced[i]) & 3;
                err = err.wrapping_add(
                    (squarei(block_r[sel] as i32 - r)
                        + squarei(block_g[sel] as i32 - g)
                        + squarei(block_b[sel] as i32 - b)) as u32,
                );
            }
            err
        } else if flags & ENCODE_BC1_USE_FASTER_MSE_EVAL != 0 {
            self.bc1_find_sels4_fasterr(pixels, lr, lg, lb, hr, hg, hb, sels, cur_err)
        } else if flags & ENCODE_BC1_USE_FULL_MSE_EVAL != 0 {
            self.bc1_find_sels4_fullerr(pixels, lr, lg, lb, hr, hg, hb, sels, cur_err)
        } else {
            self.bc1_find_sels4_check2_err(pixels, lr, lg, lb, hr, hg, hb, sels, cur_err)
        }
    }

    fn bc1_find_sels3_fullerr(
        &self,
        use_black: bool,
        pixels: &[Color32; 16],
        lr: i32,
        lg: i32,
        lb: i32,
        hr: i32,
        hg: i32,
        hb: i32,
        sels: &mut [u8; 16],
        cur_err: u32,
        force_selectors: Option<&[u8; 16]>,
    ) -> u32 {
        // (The fourth colour is black.)
        let [block_r, block_g, block_b] = self.bc1_get_block_colors3(
            lr as u32, lg as u32, lb as u32, hr as u32, hg as u32, hb as u32,
        );

        let mut total_err = 0u32;

        if let Some(forced) = force_selectors {
            *sels = *forced;

            for i in 0..16 {
                let (r, g, b) = (
                    i32::from(pixels[i][0]),
                    i32::from(pixels[i][1]),
                    i32::from(pixels[i][2]),
                );
                let sel = usize::from(forced[i]) & 3;
                total_err = total_err.wrapping_add(
                    (squarei(block_r[sel] as i32 - r)
                        + squarei(block_g[sel] as i32 - g)
                        + squarei(block_b[sel] as i32 - b)) as u32,
                );
            }
        } else {
            for i in 0..16 {
                let (r, g, b) = (
                    i32::from(pixels[i][0]),
                    i32::from(pixels[i][1]),
                    i32::from(pixels[i][2]),
                );

                let mut best_err = (squarei(block_r[0] as i32 - r)
                    + squarei(block_g[0] as i32 - g)
                    + squarei(block_b[0] as i32 - b)) as u32;
                let mut best_sel = 0u8;

                let err1 = (squarei(block_r[1] as i32 - r)
                    + squarei(block_g[1] as i32 - g)
                    + squarei(block_b[1] as i32 - b)) as u32;
                if err1 < best_err {
                    best_err = err1;
                    best_sel = 1;
                }

                let err2 = (squarei(block_r[2] as i32 - r)
                    + squarei(block_g[2] as i32 - g)
                    + squarei(block_b[2] as i32 - b)) as u32;
                if err2 < best_err {
                    best_err = err2;
                    best_sel = 2;
                }

                if use_black {
                    let err3 = (squarei(r) + squarei(g) + squarei(b)) as u32;
                    if err3 < best_err {
                        best_err = err3;
                        best_sel = 3;
                    }
                }

                total_err = total_err.wrapping_add(best_err);
                if total_err >= cur_err {
                    return total_err;
                }

                sels[i] = best_sel;
            }
        }

        total_err
    }

    /// The endpoints a solid block of the average colour would have, in 3-colour mode (`half`)
    /// or 4-colour mode: lr, lg, lb, hr, hg, hb.
    fn match_endpoints(&self, half: bool, avg: [i32; 3]) -> [i32; 6] {
        let (table5, table6) = if half {
            (&self.match5_half, &self.match6_half)
        } else {
            (&self.match5_equals_1, &self.match6_equals_1)
        };
        let (r, g, b) = (
            table5[avg[0] as usize & 255],
            table6[avg[1] as usize & 255],
            table5[avg[2] as usize & 255],
        );
        [
            i32::from(r.hi),
            i32::from(g.hi),
            i32::from(b.hi),
            i32::from(r.lo),
            i32::from(g.lo),
            i32::from(b.lo),
        ]
    }
}

const ADJACENT_VOXELS: [[i8; 4]; 16] = [
    [1, 0, 0, 3],   // 0
    [0, 1, 0, 4],   // 1
    [0, 0, 1, 5],   // 2
    [-1, 0, 0, 0],  // 3
    [0, -1, 0, 1],  // 4
    [0, 0, -1, 2],  // 5
    [1, 1, 0, 9],   // 6
    [1, 0, 1, 10],  // 7
    [0, 1, 1, 11],  // 8
    [-1, -1, 0, 6], // 9
    [-1, 0, -1, 7], // 10
    [0, -1, -1, 8], // 11
    [-1, 1, 0, 13], // 12
    [1, -1, 0, 12], // 13
    [0, -1, 1, 15], // 14
    [0, 1, -1, 14], // 15
];

impl Rgbcx {
    fn try_3color_block_useblack(
        &self,
        pixels: &[Color32; 16],
        flags: u32,
        cur_err: &mut u32,
        results: &mut Bc1EncodeResults,
        force_selectors: Option<&[u8; 16]>,
    ) -> bool {
        let (mut total_r, mut total_g, mut total_b) = (0i32, 0i32, 0i32);
        let (mut max_r, mut max_g, mut max_b) = (0i32, 0i32, 0i32);
        let (mut min_r, mut min_g, mut min_b) = (255i32, 255i32, 255i32);
        let mut total_pixels = 0i32;
        for pixel in pixels {
            let (r, g, b) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );
            if (r | g | b) < 4 {
                continue;
            }

            max_r = max_r.max(r);
            max_g = max_g.max(g);
            max_b = max_b.max(b);
            min_r = min_r.min(r);
            min_g = min_g.min(g);
            min_b = min_b.min(b);
            total_r += r;
            total_g += g;
            total_b += b;

            total_pixels += 1;
        }

        if total_pixels == 0 {
            return false;
        }

        let half_total_pixels = total_pixels >> 1;
        let avg_r = (total_r + half_total_pixels) / total_pixels;
        let avg_g = (total_g + half_total_pixels) / total_pixels;
        let avg_b = (total_b + half_total_pixels) / total_pixels;

        let (mut low_c, mut high_c) = (0usize, 0usize);

        let mut icov = [0i32; 6];
        for pixel in pixels {
            let (mut r, mut g, mut b) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );

            if (r | g | b) < 4 {
                continue;
            }

            r -= avg_r;
            g -= avg_g;
            b -= avg_b;

            icov[0] += r * r;
            icov[1] += r * g;
            icov[2] += r * b;
            icov[3] += g * g;
            icov[4] += g * b;
            icov[5] += b * b;
        }

        let mut cov = [0f32; 6];
        for i in 0..6 {
            cov[i] = icov[i] as f32 * (1.0f32 / 255.0f32);
        }

        let mut xr = (max_r - min_r) as f32;
        let mut xg = (max_g - min_g) as f32;
        let mut xb = (max_b - min_b) as f32;

        if icov[2] < 0 {
            xr = -xr;
        }

        if icov[4] < 0 {
            xg = -xg;
        }

        for _ in 0..4 {
            let r = xr * cov[0] + xg * cov[1] + xb * cov[2];
            let g = xr * cov[1] + xg * cov[3] + xb * cov[4];
            let b = xr * cov[2] + xg * cov[4] + xb * cov[5];
            xr = r;
            xg = g;
            xb = b;
        }

        let k = maximum3(xr.abs(), xg.abs(), xb.abs());
        let (mut saxis_r, mut saxis_g, mut saxis_b) = (306i32, 601i32, 117i32);
        if k >= 2.0 {
            let m = 1024.0f32 / k;
            saxis_r = f2i(xr * m);
            saxis_g = f2i(xg * m);
            saxis_b = f2i(xb * m);
        }

        let (mut low_dot, mut high_dot) = (i32::MAX, i32::MIN);
        for (i, pixel) in pixels.iter().enumerate() {
            let (r, g, b) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );

            if (r | g | b) < 4 {
                continue;
            }

            let dot = r
                .wrapping_mul(saxis_r)
                .wrapping_add(g.wrapping_mul(saxis_g))
                .wrapping_add(b.wrapping_mul(saxis_b));
            if dot < low_dot {
                low_dot = dot;
                low_c = i;
            }
            if dot > high_dot {
                high_dot = dot;
                high_c = i;
            }
        }

        let mut lr = i32::from(to_5(u32::from(pixels[low_c][0])));
        let mut lg = i32::from(to_6(u32::from(pixels[low_c][1])));
        let mut lb = i32::from(to_5(u32::from(pixels[low_c][2])));

        let mut hr = i32::from(to_5(u32::from(pixels[high_c][0])));
        let mut hg = i32::from(to_6(u32::from(pixels[high_c][1])));
        let mut hb = i32::from(to_5(u32::from(pixels[high_c][2])));

        let mut trial_sels = [0u8; 16];
        let mut trial_err = self.bc1_find_sels3_fullerr(
            true,
            pixels,
            lr,
            lg,
            lb,
            hr,
            hg,
            hb,
            &mut trial_sels,
            u32::MAX,
            force_selectors,
        );

        if trial_err != 0 {
            let total_ls_passes = if flags & ENCODE_BC1_TWO_LEAST_SQUARES_PASSES != 0 {
                2
            } else {
                1
            };
            for _ in 0..total_ls_passes {
                let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                let [lr2, lg2, lb2, hr2, hg2, hb2] = if !compute_least_squares_endpoints3_rgb(
                    true,
                    pixels,
                    &trial_sels,
                    &mut xl,
                    &mut xh,
                ) {
                    self.match_endpoints(true, [avg_r, avg_g, avg_b])
                } else {
                    // (As the C++ calls it: what comes of xl goes to the high endpoint.)
                    let [hr2, hg2, hb2, lr2, lg2, lb2] = precise_round_565(&xl, &xh);
                    [lr2, lg2, lb2, hr2, hg2, hb2]
                };

                if lr == lr2 && lg == lg2 && lb == lb2 && hr == hr2 && hg == hg2 && hb == hb2 {
                    break;
                }

                let mut trial_sels2 = [0u8; 16];
                let trial_err2 = self.bc1_find_sels3_fullerr(
                    true,
                    pixels,
                    lr2,
                    lg2,
                    lb2,
                    hr2,
                    hg2,
                    hb2,
                    &mut trial_sels2,
                    trial_err,
                    force_selectors,
                );

                if trial_err2 < trial_err {
                    trial_err = trial_err2;
                    lr = lr2;
                    lg = lg2;
                    lb = lb2;
                    hr = hr2;
                    hg = hg2;
                    hb = hb2;
                    trial_sels = trial_sels2;
                } else {
                    break;
                }
            }
        }

        if trial_err < *cur_err {
            results.is_3color = true;
            results.lr = lr;
            results.lg = lg;
            results.lb = lb;
            results.hr = hr;
            results.hg = hg;
            results.hb = hb;
            results.sels = trial_sels;

            *cur_err = trial_err;

            return true;
        }

        false
    }

    fn try_3color_block(
        &self,
        pixels: &[Color32; 16],
        flags: u32,
        cur_err: &mut u32,
        avg: [i32; 3],
        mut lr: i32,
        mut lg: i32,
        mut lb: i32,
        mut hr: i32,
        mut hg: i32,
        mut hb: i32,
        total: [i32; 3],
        total_orderings_to_try: u32,
        results: &mut Bc1EncodeResults,
        force_selectors: Option<&[u8; 16]>,
    ) -> bool {
        if let Some(forced) = force_selectors {
            if forced.contains(&3) {
                return false;
            }
        }

        let mut trial_sels = [0u8; 16];
        let mut trial_err = self.bc1_find_sels3_fullerr(
            false,
            pixels,
            lr,
            lg,
            lb,
            hr,
            hg,
            hb,
            &mut trial_sels,
            u32::MAX,
            force_selectors,
        );

        if trial_err != 0 {
            let total_ls_passes = if flags & ENCODE_BC1_TWO_LEAST_SQUARES_PASSES != 0 {
                2
            } else {
                1
            };
            for _ in 0..total_ls_passes {
                let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                let [lr2, lg2, lb2, hr2, hg2, hb2] = if !compute_least_squares_endpoints3_rgb(
                    false,
                    pixels,
                    &trial_sels,
                    &mut xl,
                    &mut xh,
                ) {
                    self.match_endpoints(true, avg)
                } else {
                    let [hr2, hg2, hb2, lr2, lg2, lb2] = precise_round_565(&xl, &xh);
                    [lr2, lg2, lb2, hr2, hg2, hb2]
                };

                if lr == lr2 && lg == lg2 && lb == lb2 && hr == hr2 && hg == hg2 && hb == hb2 {
                    break;
                }

                let mut trial_sels2 = [0u8; 16];
                let trial_err2 = self.bc1_find_sels3_fullerr(
                    false,
                    pixels,
                    lr2,
                    lg2,
                    lb2,
                    hr2,
                    hg2,
                    hb2,
                    &mut trial_sels2,
                    trial_err,
                    force_selectors,
                );

                if trial_err2 < trial_err {
                    trial_err = trial_err2;
                    lr = lr2;
                    lg = lg2;
                    lb = lb2;
                    hr = hr2;
                    hg = hg2;
                    hb = hb2;
                    trial_sels = trial_sels2;
                } else {
                    break;
                }
            }
        }

        if trial_err != 0
            && flags & ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS != 0
            && total_orderings_to_try != 0
        {
            let mut hist = [0u8; 3];
            for i in 0..16 {
                hist[usize::from(trial_sels[i]) % 3] += 1;
            }

            let orig_total_order_index =
                lookup_total_ordering_index3(&self.total_ordering3_hash, &hist) as usize;

            let sums = sums_along_axis(pixels, lr, lg, lb, hr, hg, hb, total);

            let q_total = if flags & ENCODE_BC1_EXHAUSTIVE != 0 {
                NUM_UNIQUE_TOTAL_ORDERINGS3 as u32
            } else {
                total_orderings_to_try.min(MAX_TOTAL_ORDERINGS3)
            };
            for q in 0..q_total {
                let s = if flags & ENCODE_BC1_EXHAUSTIVE != 0 {
                    q
                } else {
                    u32::from(BEST_TOTAL_ORDERINGS3[orig_total_order_index][q as usize])
                };

                let [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb] = if s
                    == TOTAL_ORDER_3_0_16
                    || s == TOTAL_ORDER_3_1_16
                    || s == TOTAL_ORDER_3_2_16
                {
                    self.match_endpoints(true, avg)
                } else {
                    let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                    compute_least_squares_endpoints3_rgb_sums(
                        &mut xl,
                        &mut xh,
                        total,
                        self.selector_factors3[s as usize],
                        s as usize,
                        &sums,
                    );

                    let [trial_hr, trial_hg, trial_hb, trial_lr, trial_lg, trial_lb] =
                        precise_round_565(&xl, &xh);
                    [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb]
                };

                let mut trial_sels2 = [0u8; 16];
                let trial_err2 = self.bc1_find_sels3_fullerr(
                    false,
                    pixels,
                    trial_lr,
                    trial_lg,
                    trial_lb,
                    trial_hr,
                    trial_hg,
                    trial_hb,
                    &mut trial_sels2,
                    u32::MAX,
                    force_selectors,
                );

                if trial_err2 < trial_err {
                    trial_err = trial_err2;

                    lr = trial_lr;
                    lg = trial_lg;
                    lb = trial_lb;

                    hr = trial_hr;
                    hg = trial_hg;
                    hb = trial_hb;

                    trial_sels = trial_sels2;
                }
            }
        }

        if trial_err < *cur_err {
            results.is_3color = true;
            results.lr = lr;
            results.lg = lg;
            results.lb = lb;
            results.hr = hr;
            results.hg = hg;
            results.hb = hb;
            results.sels = trial_sels;

            *cur_err = trial_err;

            return true;
        }

        false
    }

    /// Encodes a 4x4 block of RGBX (X=ignored) pixels to BC1 format.
    ///
    /// This is the simplified interface for BC1 encoding, which accepts a level parameter and
    /// converts that to the best overall flags. The pixels are in RGBA format, where R is
    /// first in memory. The BC1 encoder completely ignores the alpha channel (i.e. there is no
    /// punchthrough alpha support). This is the recommended function to use for BC1 encoding;
    /// it configures the encoder for you in the best possible way (on average). Note that the
    /// 3 color modes won't be used at all until level 5 or higher. No transparency supported,
    /// however if you set `use_transparent_texels_for_black` to true the encoder will use
    /// transparent selectors on very dark/black texels to reduce MSE.
    pub fn encode_bc1(
        &self,
        level: u32,
        dst: &mut [u8; 8],
        pixels: &[Color32; 16],
        allow_3color: bool,
        allow_transparent_texels_for_black: bool,
        force_selectors: Option<&[u8; 16]>,
    ) {
        let flags;
        let (mut total_orderings4, mut total_orderings3) = (1u32, 1u32);

        let three_color = if allow_3color {
            ENCODE_BC1_USE_3COLOR_BLOCKS
        } else {
            0
        } | if allow_transparent_texels_for_black {
            ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS
        } else {
            0
        };
        let rounds = |count: u32| count << ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_SHIFT;
        const THOROUGH: u32 = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
            | ENCODE_BC1_USE_FULL_MSE_EVAL
            | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
            | ENCODE_BC1_USE_6_POWER_ITERS
            | ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS;

        match level {
            0 => {
                // Faster/higher quality than stb_dxt default.
                flags = ENCODE_BC1_BOUNDING_BOX_INT;
            }
            1 => {
                // Faster/higher quality than stb_dxt default. A bit higher average quality vs. mode 0.
                flags = ENCODE_BC1_USE_2D_LS;
            }
            2 => {
                // On average mode 2 is a little weaker than modes 0/1, but it's stronger on outliers (very tough textures).
                // Slightly stronger than stb_dxt.
                flags = 0;
            }
            3 => {
                // Slightly stronger than stb_dxt HIGHQUAL.
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES;
            }
            4 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_FULL_MSE_EVAL
                    | ENCODE_BC1_USE_6_POWER_ITERS;
            }
            6 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_FASTER_MSE_EVAL
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
            }
            7 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_FASTER_MSE_EVAL
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 4;
            }
            8 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_FASTER_MSE_EVAL
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 8;
            }
            9 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 11;
                total_orderings3 = 3;
            }
            10 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 20;
                total_orderings3 = 8;
            }
            11 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 28;
                total_orderings3 = 16;
            }
            12 => {
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                    | three_color;
                total_orderings4 = 32;
                total_orderings3 = 32;
            }
            13 => {
                flags = THOROUGH | rounds(20) | three_color;
                total_orderings4 = 32;
                total_orderings3 = 32;
            }
            14 => {
                flags = THOROUGH | rounds(32) | three_color;
                total_orderings4 = 32;
                total_orderings3 = 32;
            }
            15 => {
                flags = THOROUGH | rounds(32) | three_color;
                total_orderings4 = (((32 + MAX_TOTAL_ORDERINGS4) / 2) + 32) / 2;
                total_orderings3 = 32;
            }
            16 => {
                flags = THOROUGH | rounds(256) | three_color;
                total_orderings4 = (32 + MAX_TOTAL_ORDERINGS4) / 2;
                total_orderings3 = 32;
            }
            17 => {
                flags = THOROUGH | rounds(256) | three_color;
                total_orderings4 = MAX_TOTAL_ORDERINGS4;
                total_orderings3 = 32;
            }
            18 => {
                flags = THOROUGH | ENCODE_BC1_ITERATIVE | rounds(256) | three_color;
                total_orderings4 = MAX_TOTAL_ORDERINGS4;
                total_orderings3 = 32;
            }
            19 => {
                // This hidden mode is *extremely* slow and abuses the encoder. It's just for testing/training.
                flags = THOROUGH
                    | ENCODE_BC1_EXHAUSTIVE
                    | ENCODE_BC1_ITERATIVE
                    | rounds(256)
                    | three_color;
                total_orderings4 = 32;
                total_orderings3 = 32;
            }
            _ => {
                // 5 (and any level it doesn't know): stb_dxt HIGHQUAL + permit 3 color (if it's enabled).
                flags = ENCODE_BC1_TWO_LEAST_SQUARES_PASSES
                    | ENCODE_BC1_USE_FASTER_MSE_EVAL
                    | three_color;
            }
        }

        self.encode_bc1_with_flags(
            dst,
            pixels,
            flags,
            total_orderings4,
            total_orderings3,
            force_selectors,
        );
    }

    /// The first guess at a block's endpoints: lr, lg, lb, hr, hg, hb.
    fn encode_bc1_pick_initial(
        &self,
        pixels: &[Color32; 16],
        flags: u32,
        grayscale_flag: bool,
        mut min: [i32; 3],
        mut max: [i32; 3],
        avg: [i32; 3],
        total: [i32; 3],
    ) -> [i32; 6] {
        if grayscale_flag {
            let fr = u32::from(pixels[0][0]);

            // Grayscale blocks are a common enough case to specialize.
            if max[0] - min[0] < 2 {
                let (five, six) = (i32::from(to_5(fr)), i32::from(to_6(fr)));
                [five, six, five, five, six, five]
            } else {
                let (low5, low6) = (
                    i32::from(to_5(min[0] as u32)),
                    i32::from(to_6(min[0] as u32)),
                );
                let (high5, high6) = (
                    i32::from(to_5(max[0] as u32)),
                    i32::from(to_6(max[0] as u32)),
                );
                [low5, low6, low5, high5, high6, high5]
            }
        } else if flags & ENCODE_BC1_USE_2D_LS != 0 {
            //  2D Least Squares approach from Humus's example, with added inset and optimal rounding.
            let (mut big_chan, mut min_chan_val, mut max_chan_val) = (0usize, min[0], max[0]);
            if max[1] - min[1] > max_chan_val - min_chan_val {
                big_chan = 1;
                min_chan_val = min[1];
                max_chan_val = max[1];
            }

            if max[2] - min[2] > max_chan_val - min_chan_val {
                big_chan = 2;
                min_chan_val = min[2];
                max_chan_val = max[2];
            }

            // The sums of each channel times the big one. The C++ has this three times over,
            // once for each channel that can be the big one; `others` are the other two, in
            // the order it takes them.
            let mut sum_xy = [0i32; 3];
            for pixel in pixels {
                let x = i32::from(pixel[big_chan]);
                for c in 0..3 {
                    sum_xy[c] += x * i32::from(pixel[c]);
                }
            }

            let others = match big_chan {
                0 => [1, 2],
                1 => [0, 2],
                _ => [0, 1],
            };

            let sum_x = total[big_chan];
            let sum_x2 = sum_xy[big_chan];

            let mut div = (16 * sum_x2 - sum_x * sum_x) as f32;
            let mut slope = [0.0f32; 2];
            if div.abs() > 1e-8f32 {
                div = 1.0f32 / div;
                for (k, &c) in others.iter().enumerate() {
                    slope[k] = (16 * sum_xy[c] - sum_x * total[c]) as f32 * div;
                }
            }

            let (mut l, mut h) = ([0f32; 3], [0f32; 3]);
            let mut intercept = [0f32; 2];
            for (k, &c) in others.iter().enumerate() {
                intercept[k] = (total[c] as f32 - slope[k] * sum_x as f32) / 16.0f32;
            }
            for (k, &c) in others.iter().enumerate() {
                l[c] = intercept[k] + slope[k] * min_chan_val as f32;
            }
            for (k, &c) in others.iter().enumerate() {
                h[c] = intercept[k] + slope[k] * max_chan_val as f32;
            }

            let delta = [h[others[0]] - l[others[0]], h[others[1]] - l[others[1]]];
            for (k, &c) in others.iter().enumerate() {
                h[c] = l[c] + delta[k] * (15.0f32 / 16.0f32);
            }
            for (k, &c) in others.iter().enumerate() {
                l[c] += delta[k] * (1.0f32 / 16.0f32);
            }

            let d = (max_chan_val - min_chan_val) as f32;
            let fmin_chan_val = min_chan_val as f32 + d * (1.0f32 / 16.0f32);
            let fmax_chan_val = min_chan_val as f32 + d * (15.0f32 / 16.0f32);

            l[big_chan] = fmin_chan_val;
            h[big_chan] = fmax_chan_val;

            precise_round_565_noscale(l, h)
        } else if flags & ENCODE_BC1_BOUNDING_BOX != 0 {
            // Algorithm from icbc.h compress_dxt1_fast()
            let (mut l, mut h) = ([0f32; 3], [0f32; 3]);
            for c in 0..3 {
                l[c] = min[c] as f32 * (1.0f32 / 255.0f32);
            }
            for c in 0..3 {
                h[c] = max[c] as f32 * (1.0f32 / 255.0f32);
            }

            let bias = 8.0f32 / 255.0f32;
            let mut inset = [0f32; 3];
            for c in 0..3 {
                inset[c] = (h[c] - l[c] - bias) * (1.0f32 / 16.0f32);
            }

            for c in 0..3 {
                l[c] = clampf(l[c] + inset[c], 0.0, 1.0);
            }
            for c in 0..3 {
                h[c] = clampf(h[c] - inset[c], 0.0, 1.0);
            }

            let (mut icov_xz, mut icov_yz) = (0i32, 0i32);
            for pixel in pixels {
                let r = i32::from(pixel[0]) - avg[0];
                let g = i32::from(pixel[1]) - avg[1];
                let b = i32::from(pixel[2]) - avg[2];
                icov_xz += r * b;
                icov_yz += g * b;
            }

            if icov_xz < 0 {
                std::mem::swap(&mut l[0], &mut h[0]);
            }

            if icov_yz < 0 {
                std::mem::swap(&mut l[1], &mut h[1]);
            }

            precise_round_565(&l, &h)
        } else if flags & ENCODE_BC1_BOUNDING_BOX_INT != 0 {
            // Algorithm from icbc.h compress_dxt1_fast(), but converted to integer.
            let inset = [
                (max[0] - min[0] - 8) >> 4,
                (max[1] - min[1] - 8) >> 4,
                (max[2] - min[2] - 8) >> 4,
            ];

            for c in 0..3 {
                min[c] += inset[c];
            }
            if (min[0] | min[1] | min[2]) as u32 > 255 {
                for c in 0..3 {
                    min[c] = clampi(min[c], 0, 255);
                }
            }

            for c in 0..3 {
                max[c] -= inset[c];
            }
            if (max[0] | max[1] | max[2]) as u32 > 255 {
                for c in 0..3 {
                    max[c] = clampi(max[c], 0, 255);
                }
            }

            let (mut icov_xz, mut icov_yz) = (0i32, 0i32);
            for pixel in pixels {
                let r = i32::from(pixel[0]) - avg[0];
                let g = i32::from(pixel[1]) - avg[1];
                let b = i32::from(pixel[2]) - avg[2];
                icov_xz += r * b;
                icov_yz += g * b;
            }

            let (mut x0, mut y0, mut x1, mut y1) = (min[0], min[1], max[0], max[1]);

            if icov_xz < 0 {
                std::mem::swap(&mut x0, &mut x1);
            }

            if icov_yz < 0 {
                std::mem::swap(&mut y0, &mut y1);
            }

            [
                i32::from(to_5(x0 as u32)),
                i32::from(to_6(y0 as u32)),
                i32::from(to_5(min[2] as u32)),
                i32::from(to_5(x1 as u32)),
                i32::from(to_6(y1 as u32)),
                i32::from(to_5(max[2] as u32)),
            ]
        } else {
            // Select 2 colors along the principle axis. (There must be a faster/simpler way.)
            let mut icov = [0i32; 6];
            for pixel in pixels {
                let r = i32::from(pixel[0]) - avg[0];
                let g = i32::from(pixel[1]) - avg[1];
                let b = i32::from(pixel[2]) - avg[2];
                icov[0] += r * r;
                icov[1] += r * g;
                icov[2] += r * b;
                icov[3] += g * g;
                icov[4] += g * b;
                icov[5] += b * b;
            }

            let (mut saxis_r, mut saxis_g, mut saxis_b) = (306i32, 601i32, 117i32);

            let mut xr = (max[0] - min[0]) as f32;
            let mut xg = (max[1] - min[1]) as f32;
            let mut xb = (max[2] - min[2]) as f32;

            if icov[2] < 0 {
                xr = -xr;
            }

            if icov[4] < 0 {
                xg = -xg;
            }

            let mut cov = [0f32; 6];
            for i in 0..6 {
                cov[i] = icov[i] as f32 * (1.0f32 / 255.0f32);
            }

            let total_power_iters = if flags & ENCODE_BC1_USE_6_POWER_ITERS != 0 {
                6
            } else {
                4
            };
            for _ in 0..total_power_iters {
                let r = xr * cov[0] + xg * cov[1] + xb * cov[2];
                let g = xr * cov[1] + xg * cov[3] + xb * cov[4];
                let b = xr * cov[2] + xg * cov[4] + xb * cov[5];
                xr = r;
                xg = g;
                xb = b;
            }

            let k = maximum3(xr.abs(), xg.abs(), xb.abs());
            if k >= 2.0 {
                let m = 2048.0f32 / k;
                saxis_r = f2i(xr * m);
                saxis_g = f2i(xg * m);
                saxis_b = f2i(xb * m);
            }

            let (mut low_dot, mut high_dot) = (i32::MAX, i32::MIN);

            saxis_r = ((saxis_r as u32) << 4) as i32;
            saxis_g = ((saxis_g as u32) << 4) as i32;
            saxis_b = ((saxis_b as u32) << 4) as i32;

            for (i, pixel) in pixels.iter().enumerate() {
                let dot = (i32::from(pixel[0])
                    .wrapping_mul(saxis_r)
                    .wrapping_add(i32::from(pixel[1]).wrapping_mul(saxis_g))
                    .wrapping_add(i32::from(pixel[2]).wrapping_mul(saxis_b))
                    & !0xF)
                    .wrapping_add(i as i32);
                low_dot = low_dot.min(dot);
                high_dot = high_dot.max(dot);
            }
            let low_c = (low_dot & 15) as usize;
            let high_c = (high_dot & 15) as usize;

            [
                i32::from(to_5(u32::from(pixels[low_c][0]))),
                i32::from(to_6(u32::from(pixels[low_c][1]))),
                i32::from(to_5(u32::from(pixels[low_c][2]))),
                i32::from(to_5(u32::from(pixels[high_c][0]))),
                i32::from(to_6(u32::from(pixels[high_c][1]))),
                i32::from(to_5(u32::from(pixels[high_c][2]))),
            ]
        }
    }

    // From icbc's high quality mode.
    fn encode_bc1_endpoint_search(
        &self,
        pixels: &[Color32; 16],
        any_black_pixels: bool,
        flags: u32,
        results: &mut Bc1EncodeResults,
        mut cur_err: u32,
        force_selectors: Option<&[u8; 16]>,
    ) {
        let (mut prev_improvement_index, mut forbidden_direction) = (0i32, -1i32);

        let endpoint_search_rounds = ((flags & ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_MASK)
            >> ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_SHIFT) as i32;
        for i in 0..endpoint_search_rounds {
            if forbidden_direction == (i & 31) {
                continue;
            }

            let voxel = ADJACENT_VOXELS[(i & 15) as usize];
            let delta = [
                i32::from(voxel[0]),
                i32::from(voxel[1]),
                i32::from(voxel[2]),
            ];

            let (
                mut trial_lr,
                mut trial_lg,
                mut trial_lb,
                mut trial_hr,
                mut trial_hg,
                mut trial_hb,
            ) = (
                results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
            );

            if (i >> 4) & 1 != 0 {
                trial_lr = clampi(trial_lr + delta[0], 0, 31);
                trial_lg = clampi(trial_lg + delta[1], 0, 63);
                trial_lb = clampi(trial_lb + delta[2], 0, 31);
            } else {
                trial_hr = clampi(trial_hr + delta[0], 0, 31);
                trial_hg = clampi(trial_hg + delta[1], 0, 63);
                trial_hb = clampi(trial_hb + delta[2], 0, 31);
            }

            let mut trial_sels = [0u8; 16];

            let trial_err = if results.is_3color {
                self.bc1_find_sels3_fullerr(
                    any_black_pixels && flags & ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS != 0,
                    pixels,
                    trial_lr,
                    trial_lg,
                    trial_lb,
                    trial_hr,
                    trial_hg,
                    trial_hb,
                    &mut trial_sels,
                    cur_err,
                    force_selectors,
                )
            } else {
                self.bc1_find_sels4(
                    flags,
                    pixels,
                    trial_lr,
                    trial_lg,
                    trial_lb,
                    trial_hr,
                    trial_hg,
                    trial_hb,
                    &mut trial_sels,
                    cur_err,
                    force_selectors,
                )
            };

            if trial_err < cur_err {
                cur_err = trial_err;

                forbidden_direction = i32::from(voxel[3]) | (i & 16);

                results.lr = trial_lr;
                results.lg = trial_lg;
                results.lb = trial_lb;
                results.hr = trial_hr;
                results.hg = trial_hg;
                results.hb = trial_hb;

                results.sels = trial_sels;

                prev_improvement_index = i;
            }

            if i - prev_improvement_index > 32 {
                break;
            }
        }
    }
}

impl Rgbcx {
    /// Encodes a 4x4 block of RGBX (X=ignored) pixels to BC1 format: the low-level interface,
    /// with the encoder's flags and how many total orderings to try given directly.
    ///
    /// `total_orderings_to_try` (for 4-colour blocks, 1 to [`MAX_TOTAL_ORDERINGS4`]) and
    /// `total_orderings_to_try3` (for 3-colour blocks, 0 to [`MAX_TOTAL_ORDERINGS3`]) are only
    /// used with [`ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS`]. With `force_selectors`, the block
    /// keeps those selectors (in linear order: 0 the low colour, 3 the high) and only its
    /// endpoints are chosen.
    pub fn encode_bc1_with_flags(
        &self,
        dst: &mut [u8; 8],
        pixels: &[Color32; 16],
        flags: u32,
        total_orderings_to_try: u32,
        total_orderings_to_try3: u32,
        force_selectors: Option<&[u8; 16]>,
    ) {
        let (fr, fg, fb) = (
            u32::from(pixels[0][0]),
            u32::from(pixels[0][1]),
            u32::from(pixels[0][2]),
        );

        let mut j = 15;
        while j >= 1 {
            if u32::from(pixels[j][0]) != fr
                || u32::from(pixels[j][1]) != fg
                || u32::from(pixels[j][2]) != fb
            {
                break;
            }
            j -= 1;
        }

        if j == 0 {
            self.encode_bc1_solid_block(
                dst,
                fr,
                fg,
                fb,
                flags
                    & (ENCODE_BC1_USE_3COLOR_BLOCKS
                        | ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS)
                    != 0,
            );
            return;
        }

        let mut total = [fr as i32, fg as i32, fb as i32];
        let mut max = total;
        let mut min = total;

        let mut grayscale_flag = fr == fg && fr == fb;
        let mut any_black_pixels = (fr | fg | fb) < 4;

        for pixel in &pixels[1..] {
            let (r, g, b) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );

            grayscale_flag &= r == g && r == b;
            any_black_pixels |= (r | g | b) < 4;

            for (c, value) in [r, g, b].into_iter().enumerate() {
                max[c] = max[c].max(value);
                min[c] = min[c].min(value);
                total[c] += value;
            }
        }

        let avg = [
            (total[0] + 8) >> 4,
            (total[1] + 8) >> 4,
            (total[2] + 8) >> 4,
        ];

        let mut results = Bc1EncodeResults::default();
        let mut orig = [0i32; 6];

        let needs_block_error = flags
            & (ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS
                | ENCODE_BC1_USE_3COLOR_BLOCKS
                | ENCODE_BC1_USE_FULL_MSE_EVAL
                | ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_MASK)
            != 0
            || (any_black_pixels && flags & ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS != 0);

        let mut cur_err = u32::MAX;

        if !needs_block_error {
            let [lr, lg, lb, hr, hg, hb] =
                self.encode_bc1_pick_initial(pixels, flags, grayscale_flag, min, max, avg, total);
            (
                results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
            ) = (lr, lg, lb, hr, hg, hb);

            self.bc1_find_sels4_noerr(
                pixels,
                lr,
                lg,
                lb,
                hr,
                hg,
                hb,
                &mut results.sels,
                force_selectors,
            );

            let total_ls_passes = if flags & ENCODE_BC1_TWO_LEAST_SQUARES_PASSES != 0 {
                2
            } else {
                1
            };
            for _ in 0..total_ls_passes {
                let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                let [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb] =
                    if !compute_least_squares_endpoints4_rgb(
                        pixels,
                        &results.sels,
                        &mut xl,
                        &mut xh,
                        total,
                    ) {
                        // All selectors equal - treat it as a solid block which should always be equal or better.
                        // In high/higher quality mode, let it try again in case the optimal tables have caused the sels to diverge.
                        self.match_endpoints(false, avg)
                    } else {
                        // (As the C++ calls it: what comes of xl goes to the high endpoint.)
                        let [trial_hr, trial_hg, trial_hb, trial_lr, trial_lg, trial_lb] =
                            precise_round_565(&xl, &xh);
                        [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb]
                    };

                if results.lr == trial_lr
                    && results.lg == trial_lg
                    && results.lb == trial_lb
                    && results.hr == trial_hr
                    && results.hg == trial_hg
                    && results.hb == trial_hb
                {
                    break;
                }

                self.bc1_find_sels4_noerr(
                    pixels,
                    trial_lr,
                    trial_lg,
                    trial_lb,
                    trial_hr,
                    trial_hg,
                    trial_hb,
                    &mut results.sels,
                    force_selectors,
                );

                (
                    results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
                ) = (trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb);
            }
        } else {
            let total_rounds = if flags & ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS != 0 {
                2
            } else {
                1
            };
            for round in 0..total_rounds {
                let mut modified_flags = flags;
                if round == 1 {
                    modified_flags &= !(ENCODE_BC1_USE_2D_LS | ENCODE_BC1_BOUNDING_BOX);
                    modified_flags |= ENCODE_BC1_BOUNDING_BOX;
                }

                let mut round_sels = [0u8; 16];

                let orig_round = self.encode_bc1_pick_initial(
                    pixels,
                    modified_flags,
                    grayscale_flag,
                    min,
                    max,
                    avg,
                    total,
                );
                let [mut round_lr, mut round_lg, mut round_lb, mut round_hr, mut round_hg, mut round_hb] =
                    orig_round;

                let mut round_err = self.bc1_find_sels4(
                    flags,
                    pixels,
                    round_lr,
                    round_lg,
                    round_lb,
                    round_hr,
                    round_hg,
                    round_hb,
                    &mut round_sels,
                    u32::MAX,
                    force_selectors,
                );

                let total_ls_passes = if flags & ENCODE_BC1_TWO_LEAST_SQUARES_PASSES != 0 {
                    2
                } else {
                    1
                };
                for _ in 0..total_ls_passes {
                    let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                    let [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb] =
                        if !compute_least_squares_endpoints4_rgb(
                            pixels,
                            &round_sels,
                            &mut xl,
                            &mut xh,
                            total,
                        ) {
                            // All selectors equal - treat it as a solid block which should always be equal or better.
                            self.match_endpoints(false, avg)
                        } else {
                            let [trial_hr, trial_hg, trial_hb, trial_lr, trial_lg, trial_lb] =
                                precise_round_565(&xl, &xh);
                            [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb]
                        };

                    if round_lr == trial_lr
                        && round_lg == trial_lg
                        && round_lb == trial_lb
                        && round_hr == trial_hr
                        && round_hg == trial_hg
                        && round_hb == trial_hb
                    {
                        break;
                    }

                    let mut trial_sels = [0u8; 16];
                    let trial_err = self.bc1_find_sels4(
                        flags,
                        pixels,
                        trial_lr,
                        trial_lg,
                        trial_lb,
                        trial_hr,
                        trial_hg,
                        trial_hb,
                        &mut trial_sels,
                        round_err,
                        force_selectors,
                    );

                    if trial_err < round_err {
                        round_lr = trial_lr;
                        round_lg = trial_lg;
                        round_lb = trial_lb;

                        round_hr = trial_hr;
                        round_hg = trial_hg;
                        round_hb = trial_hb;

                        round_err = trial_err;
                        round_sels = trial_sels;
                    } else {
                        break;
                    }
                }

                if round_err <= cur_err {
                    cur_err = round_err;

                    (
                        results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
                    ) = (round_lr, round_lg, round_lb, round_hr, round_hg, round_hb);

                    orig = orig_round;

                    results.sels = round_sels;
                }
            }
        }

        if cur_err != 0 && flags & ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS != 0 {
            let total_iters = if flags & ENCODE_BC1_ITERATIVE != 0 {
                2
            } else {
                1
            };
            for _ in 0..total_iters {
                let orig_err = cur_err;

                let mut hist = [0u8; 4];
                for i in 0..16 {
                    hist[usize::from(results.sels[i]) & 3] += 1;
                }

                let orig_total_order_index =
                    lookup_total_ordering_index4(&self.total_ordering4_hash, &hist) as usize;

                let sums = sums_along_axis(
                    pixels, results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
                    total,
                );

                let q_total = if flags & ENCODE_BC1_EXHAUSTIVE != 0 {
                    NUM_UNIQUE_TOTAL_ORDERINGS4 as u32
                } else {
                    clampi(
                        total_orderings_to_try as i32,
                        MIN_TOTAL_ORDERINGS as i32,
                        MAX_TOTAL_ORDERINGS4 as i32,
                    ) as u32
                };
                for q in 0..q_total {
                    let s = if flags & ENCODE_BC1_EXHAUSTIVE != 0 {
                        q
                    } else {
                        u32::from(BEST_TOTAL_ORDERINGS4[orig_total_order_index][q as usize])
                    };

                    let [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb] = if s
                        == TOTAL_ORDER_4_0_16
                        || s == TOTAL_ORDER_4_1_16
                        || s == TOTAL_ORDER_4_2_16
                        || s == TOTAL_ORDER_4_3_16
                    {
                        self.match_endpoints(false, avg)
                    } else {
                        let (mut xl, mut xh) = ([0f32; 3], [0f32; 3]);
                        compute_least_squares_endpoints4_rgb_sums(
                            &mut xl,
                            &mut xh,
                            total,
                            self.selector_factors4[s as usize],
                            s as usize,
                            &sums,
                        );

                        let [trial_hr, trial_hg, trial_hb, trial_lr, trial_lg, trial_lb] =
                            precise_round_565(&xl, &xh);
                        [trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb]
                    };

                    let mut trial_sels = [0u8; 16];

                    let trial_err = self.bc1_find_sels4(
                        flags,
                        pixels,
                        trial_lr,
                        trial_lg,
                        trial_lb,
                        trial_hr,
                        trial_hg,
                        trial_hb,
                        &mut trial_sels,
                        cur_err,
                        force_selectors,
                    );

                    if trial_err < cur_err {
                        cur_err = trial_err;

                        (
                            results.lr, results.lg, results.lb, results.hr, results.hg, results.hb,
                        ) = (trial_lr, trial_lg, trial_lb, trial_hr, trial_hg, trial_hb);

                        results.sels = trial_sels;
                    }
                }

                if cur_err == 0 || cur_err == orig_err {
                    break;
                }
            }
        }

        if flags & (ENCODE_BC1_USE_3COLOR_BLOCKS | ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS)
            != 0
            && cur_err != 0
        {
            if flags & ENCODE_BC1_USE_3COLOR_BLOCKS != 0 {
                self.try_3color_block(
                    pixels,
                    flags,
                    &mut cur_err,
                    avg,
                    orig[0],
                    orig[1],
                    orig[2],
                    orig[3],
                    orig[4],
                    orig[5],
                    total,
                    total_orderings_to_try3,
                    &mut results,
                    force_selectors,
                );
            }

            if any_black_pixels && flags & ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS != 0 {
                self.try_3color_block_useblack(
                    pixels,
                    flags,
                    &mut cur_err,
                    &mut results,
                    force_selectors,
                );
            }
        }

        if flags & ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_MASK != 0 && cur_err != 0 {
            self.encode_bc1_endpoint_search(
                pixels,
                any_black_pixels,
                flags,
                &mut results,
                cur_err,
                force_selectors,
            );
        }

        if results.is_3color {
            bc1_encode3(
                dst,
                results.lr,
                results.lg,
                results.lb,
                results.hr,
                results.hg,
                results.hb,
                &results.sels,
            );
        } else {
            bc1_encode4(
                dst,
                results.lr,
                results.lg,
                results.lb,
                results.hr,
                results.hg,
                results.hb,
                &results.sels,
            );
        }
    }

    /// Encodes a 4x4 block of RGBA pixels to BC3 format. There are two encode_bc3() functions.
    /// The first is the recommended function, which accepts a level parameter. The second is
    /// a low-level version that allows fine control over BC1 encoding.
    pub fn encode_bc3(&self, level: u32, dst: &mut [u8; 16], pixels: &[Color32; 16]) {
        let (alpha, color) = dst.split_at_mut(8);
        encode_bc4(alpha.try_into().unwrap(), &pixels.as_flattened()[3..], 4);
        self.encode_bc1(level, color.try_into().unwrap(), pixels, false, false, None);
    }

    /// The low-level BC3 encoder: the BC1 flags and total orderings to try given directly.
    pub fn encode_bc3_with_flags(
        &self,
        dst: &mut [u8; 16],
        pixels: &[Color32; 16],
        flags: u32,
        total_orderings_to_try: u32,
    ) {
        // 3-color blocks are not allowed with BC3 (on most GPU's).
        let flags =
            flags & !(ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS | ENCODE_BC1_USE_3COLOR_BLOCKS);

        let (alpha, color) = dst.split_at_mut(8);
        encode_bc4(alpha.try_into().unwrap(), &pixels.as_flattened()[3..], 4);
        self.encode_bc1_with_flags(
            color.try_into().unwrap(),
            pixels,
            flags,
            total_orderings_to_try,
            DEFAULT_TOTAL_ORDERINGS_TO_TRY3,
            None,
        );
    }

    /// Encodes a 4x4 block of RGBA pixels to BC3 format, with the higher-quality (slower)
    /// alpha search of [`encode_bc4_hq`].
    pub fn encode_bc3_hq(
        &self,
        level: u32,
        dst: &mut [u8; 16],
        pixels: &[Color32; 16],
        alpha_search_rad: u32,
        alpha_modes: u32,
    ) {
        let (alpha, color) = dst.split_at_mut(8);
        encode_bc4_hq(
            alpha.try_into().unwrap(),
            &pixels.as_flattened()[3..],
            4,
            alpha_search_rad,
            alpha_modes,
            None,
        );
        self.encode_bc1(level, color.try_into().unwrap(), pixels, false, false, None);
    }
}

// BC3-5

/// Encodes a single channel to BC4. `pixels` holds the 16 values `stride` bytes apart (4 for
/// one channel of RGBA pixels, 1 for values by themselves).
pub fn encode_bc4(dst: &mut [u8; 8], pixels: &[u8], stride: usize) {
    let value = |i: usize| u32::from(pixels[i * stride]);

    let mut min_v = value(0);
    let mut max_v = min_v;
    for i in 1..16 {
        min_v = min_v.min(value(i));
        max_v = max_v.max(value(i));
    }

    dst[0] = max_v as u8;
    dst[1] = min_v as u8;

    if max_v == min_v {
        dst[2..].fill(0);
        return;
    }

    let delta = max_v - min_v;

    // min_v is now 0. Compute thresholds between values by scaling max_v. It's x14 because we're adding two x7 scale factors.
    let thresholds = [
        delta * 13,
        delta * 11,
        delta * 9,
        delta * 7,
        delta * 5,
        delta * 3,
        delta,
    ]
    .map(|t| t as i32);

    // BC4 floors in its divisions, which we compensate for with the 4 bias.
    // This function is optimal for all possible inputs (i.e. it outputs the same results as checking all 8 values and choosing the closest one).
    let bias = 4i32.wrapping_sub((min_v * 14) as i32);

    const TRAN: [u64; 8] = [1, 7, 6, 5, 4, 3, 2, 0];

    let mut f = 0u64;
    for i in 0..16 {
        let v = value(i) as i32 * 14 + bias;
        let rank = thresholds.iter().filter(|&&t| v >= t).count();
        f |= TRAN[rank] << (i * 3);
    }

    dst[2..].copy_from_slice(&f.to_le_bytes()[..6]);
}

#[inline]
fn bc4_get_block_values6(l: u32, h: u32) -> [u8; 8] {
    [
        l as u8,
        h as u8,
        ((l * 4 + h) / 5) as u8,
        ((l * 3 + h * 2) / 5) as u8,
        ((l * 2 + h * 3) / 5) as u8,
        ((l + h * 4) / 5) as u8,
        0,
        255,
    ]
}

#[inline]
fn bc4_get_block_values8(l: u32, h: u32) -> [u8; 8] {
    [
        l as u8,
        h as u8,
        ((l * 6 + h) / 7) as u8,
        ((l * 5 + h * 2) / 7) as u8,
        ((l * 4 + h * 3) / 7) as u8,
        ((l * 3 + h * 4) / 7) as u8,
        ((l * 2 + h * 5) / 7) as u8,
        ((l + h * 6) / 7) as u8,
    ]
}

/// The eight values a BC4 block's two endpoints stand for.
#[inline]
fn bc4_get_block_values(l: u32, h: u32) -> [u8; 8] {
    if l > h {
        bc4_get_block_values8(l, h)
    } else {
        bc4_get_block_values6(l, h)
    }
}

/// Encodes a single channel to BC4, searching the endpoints near the lowest and highest
/// values (`search_rad` either way) in the modes `mode_flag` allows ([`BC4_USE_MODE8_FLAG`],
/// [`BC4_USE_MODE6_FLAG`]). Returns the squared error of the block it chose.
pub fn encode_bc4_hq(
    dst: &mut [u8; 8],
    pixels: &[u8],
    stride: usize,
    search_rad: u32,
    mode_flag: u32,
    force_selectors: Option<&[u8; 16]>,
) -> u32 {
    let value = |i: usize| u32::from(pixels[i * stride]);

    let (mut min_val, mut max_val) = (255u32, 0u32);
    for i in 0..16 {
        min_val = min_val.min(value(i));
        max_val = max_val.max(value(i));
    }

    if min_val == max_val {
        if mode_flag & BC4_USE_MODE6_FLAG != 0 {
            dst[0] = min_val as u8;
            dst[1] = min_val as u8;
            dst[2..].fill(0);
        } else if min_val > 0 {
            // Use an 8 value encoding
            dst[0] = min_val as u8;
            dst[1] = (min_val - 1) as u8;
            dst[2..].fill(0);
        } else {
            *dst = [1, 0, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24];
        }
        return 0;
    }

    let mut best_err = u32::MAX;
    let search_rad = search_rad as i32;

    for mode in 0..2 {
        if mode_flag & (1 << mode) == 0 {
            continue;
        }

        for lo_delta in -search_rad..=search_rad {
            for hi_delta in -search_rad..=search_rad {
                let mut endpoints = [
                    clampi((max_val as i32).wrapping_add(hi_delta), 0, 255) as u8,
                    clampi((min_val as i32).wrapping_add(lo_delta), 0, 255) as u8,
                ];

                if endpoints[0] == endpoints[1] {
                    continue;
                }

                // (An "alpha6" block has its first endpoint no higher than its second.)
                let is_alpha6 = endpoints[0] <= endpoints[1];
                if (mode == 0 && is_alpha6) || (mode != 0 && !is_alpha6) {
                    endpoints.swap(0, 1);
                }

                let block_vals =
                    bc4_get_block_values(u32::from(endpoints[0]), u32::from(endpoints[1]));

                let mut trial_err = 0u32;
                let mut trial_sels = [0u8; 16];
                if let Some(forced) = force_selectors {
                    trial_sels = *forced;

                    for i in 0..16 {
                        trial_err = trial_err.wrapping_add(squarei(
                            i32::from(block_vals[usize::from(forced[i]) & 7]) - value(i) as i32,
                        ) as u32);
                    }
                } else {
                    for i in 0..16 {
                        let mut best_index_err = u32::MAX;
                        let mut best_index = 0usize;
                        for j in 0..8 {
                            let err = squarei(i32::from(block_vals[j]) - value(i) as i32) as u32;
                            if err < best_index_err {
                                best_index_err = err;
                                best_index = j;
                                if err == 0 {
                                    break;
                                }
                            }
                        }

                        trial_err = trial_err.wrapping_add(best_index_err);
                        if trial_err >= best_err {
                            break;
                        }

                        trial_sels[i] = best_index as u8;
                    }
                }

                if trial_err < best_err {
                    best_err = trial_err;

                    let mut sel_vals = 0u64;
                    for i in 0..16 {
                        sel_vals |= u64::from(trial_sels[i]) << (i * 3);
                    }

                    dst[0] = endpoints[0];
                    dst[1] = endpoints[1];
                    dst[2..].copy_from_slice(&sel_vals.to_le_bytes()[..6]);
                }
            }
        }
    }

    best_err
}

/// Encodes two channels to BC5. `chan0` and `chan1` are the places of the two channels in a
/// pixel, and `stride` the size of a pixel (4 for RGBA).
pub fn encode_bc5(dst: &mut [u8; 16], pixels: &[u8], chan0: usize, chan1: usize, stride: usize) {
    let (first, second) = dst.split_at_mut(8);
    encode_bc4(first.try_into().unwrap(), &pixels[chan0..], stride);
    encode_bc4(second.try_into().unwrap(), &pixels[chan1..], stride);
}

/// Encodes two channels to BC5 with [`encode_bc4_hq`].
pub fn encode_bc5_hq(
    dst: &mut [u8; 16],
    pixels: &[u8],
    chan0: usize,
    chan1: usize,
    stride: usize,
    alpha_search_rad: u32,
    alpha_modes: u32,
) {
    let (first, second) = dst.split_at_mut(8);
    encode_bc4_hq(
        first.try_into().unwrap(),
        &pixels[chan0..],
        stride,
        alpha_search_rad,
        alpha_modes,
        None,
    );
    encode_bc4_hq(
        second.try_into().unwrap(),
        &pixels[chan1..],
        stride,
        alpha_search_rad,
        alpha_modes,
        None,
    );
}

/// The four colours of a BC1 block. Returns true if the block uses 3 color punchthrough alpha
/// mode (its fourth colour is then transparent black).
pub fn unpack_bc1_block_colors(block: &[u8; 8], c: &mut [Color32; 4], mode: Bc1ApproxMode) -> bool {
    let l = u32::from(u16::from_le_bytes([block[0], block[1]]));
    let h = u32::from(u16::from_le_bytes([block[2], block[3]]));

    let cr0 = ((l >> 11) & 31) as i32;
    let cg0 = ((l >> 5) & 63) as i32;
    let cb0 = (l & 31) as i32;
    let r0 = (cr0 << 3) | (cr0 >> 2);
    let g0 = (cg0 << 2) | (cg0 >> 4);
    let b0 = (cb0 << 3) | (cb0 >> 2);

    let cr1 = ((h >> 11) & 31) as i32;
    let cg1 = ((h >> 5) & 63) as i32;
    let cb1 = (h & 31) as i32;
    let r1 = (cr1 << 3) | (cr1 >> 2);
    let g1 = (cg1 << 2) | (cg1 >> 4);
    let b1 = (cb1 << 3) | (cb1 >> 2);

    let rgba = |r: i32, g: i32, b: i32, a: u8| [r as u8, g as u8, b as u8, a];

    c[0] = rgba(r0, g0, b0, 255);
    c[1] = rgba(r1, g1, b1, 255);
    if l > h {
        match mode {
            Bc1ApproxMode::Ideal => {
                c[2] = rgba((r0 * 2 + r1) / 3, (g0 * 2 + g1) / 3, (b0 * 2 + b1) / 3, 255);
                c[3] = rgba((r1 * 2 + r0) / 3, (g1 * 2 + g0) / 3, (b1 * 2 + b0) / 3, 255);
            }
            Bc1ApproxMode::IdealRound4 => {
                c[2] = rgba(
                    (r0 * 2 + r1 + 1) / 3,
                    (g0 * 2 + g1 + 1) / 3,
                    (b0 * 2 + b1 + 1) / 3,
                    255,
                );
                c[3] = rgba(
                    (r1 * 2 + r0 + 1) / 3,
                    (g1 * 2 + g0 + 1) / 3,
                    (b1 * 2 + b0 + 1) / 3,
                    255,
                );
            }
            Bc1ApproxMode::NVidia => {
                c[2] = rgba(
                    interp_5_nv(cr0, cr1),
                    interp_6_nv(g0, g1),
                    interp_5_nv(cb0, cb1),
                    255,
                );
                c[3] = rgba(
                    interp_5_nv(cr1, cr0),
                    interp_6_nv(g1, g0),
                    interp_5_nv(cb1, cb0),
                    255,
                );
            }
            Bc1ApproxMode::Amd => {
                c[2] = rgba(
                    interp_5_6_amd(r0, r1),
                    interp_5_6_amd(g0, g1),
                    interp_5_6_amd(b0, b1),
                    255,
                );
                c[3] = rgba(
                    interp_5_6_amd(r1, r0),
                    interp_5_6_amd(g1, g0),
                    interp_5_6_amd(b1, b0),
                    255,
                );
            }
        }
        false
    } else {
        c[2] = match mode {
            Bc1ApproxMode::Ideal | Bc1ApproxMode::IdealRound4 => {
                rgba((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2, 255)
            }
            Bc1ApproxMode::NVidia => rgba(
                interp_half_5_nv(cr0, cr1),
                interp_half_6_nv(g0, g1),
                interp_half_5_nv(cb0, cb1),
                255,
            ),
            Bc1ApproxMode::Amd => rgba(
                interp_half_5_6_amd(r0, r1),
                interp_half_5_6_amd(g0, g1),
                interp_half_5_6_amd(b0, b1),
                255,
            ),
        };
        c[3] = [0, 0, 0, 0];
        true
    }
}

/// Unpacks a BC1 block to 16 pixels, in rows from the top. With `set_alpha` false the pixels'
/// alpha is left as it was. Returns true if the block uses 3 color punchthrough alpha mode.
pub fn unpack_bc1(
    block: &[u8; 8],
    pixels: &mut [Color32; 16],
    set_alpha: bool,
    mode: Bc1ApproxMode,
) -> bool {
    let mut c = [[0u8; 4]; 4];
    let used_punchthrough = unpack_bc1_block_colors(block, &mut c, mode);

    for (i, pixel) in pixels.iter_mut().enumerate() {
        let color = c[usize::from((block[4 + i / 4] >> ((i % 4) * 2)) & 3)];
        if set_alpha {
            *pixel = color;
        } else {
            pixel[..3].copy_from_slice(&color[..3]);
        }
    }

    used_punchthrough
}

/// Unpacks a BC4 block to 16 values, `stride` bytes apart in `pixels`.
pub fn unpack_bc4(block: &[u8; 8], pixels: &mut [u8], stride: usize) {
    let sel_values = bc4_get_block_values(u32::from(block[0]), u32::from(block[1]));

    let mut selector_bits = 0u64;
    for (i, &byte) in block[2..].iter().enumerate() {
        selector_bits |= u64::from(byte) << (i * 8);
    }

    for i in 0..16 {
        pixels[i * stride] = sel_values[((selector_bits >> (i * 3)) & 7) as usize];
    }
}

/// Unpacks a BC3 block. Returns false if the block uses 3-color punchthrough alpha mode,
/// which isn't supported on some GPU's for BC3.
pub fn unpack_bc3(block: &[u8; 16], pixels: &mut [Color32; 16], mode: Bc1ApproxMode) -> bool {
    let (alpha, color) = block.split_at(8);
    let success = !unpack_bc1(color.try_into().unwrap(), pixels, true, mode);

    unpack_bc4(
        alpha.try_into().unwrap(),
        &mut pixels.as_flattened_mut()[3..],
        4,
    );

    success
}

/// Unpacks a BC5 block into two channels of the pixels (it writes RG by default: `chan0` 0,
/// `chan1` 1, `stride` 4).
pub fn unpack_bc5(block: &[u8; 16], pixels: &mut [u8], chan0: usize, chan1: usize, stride: usize) {
    let (first, second) = block.split_at(8);
    unpack_bc4(first.try_into().unwrap(), &mut pixels[chan0..], stride);
    unpack_bc4(second.try_into().unwrap(), &mut pixels[chan1..], stride);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_solid_block_comes_back_as_its_colour() {
        let encoder = Rgbcx::default();
        // Colours that 5:6:5 holds exactly come back exactly, at every level.
        for color in [
            [0u8, 0, 0, 255],
            [255, 255, 255, 255],
            [132, 130, 132, 255],
            [8, 4, 8, 255],
        ] {
            for level in [0, 5, 10, 18] {
                let mut block = [0u8; 8];
                encoder.encode_bc1(level, &mut block, &[color; 16], false, false, None);
                let mut pixels = [[0u8; 4]; 16];
                assert!(
                    !unpack_bc1(&block, &mut pixels, true, Bc1ApproxMode::Ideal),
                    "a 4-colour block"
                );
                assert_eq!(pixels, [color; 16]);
            }
        }
    }

    #[test]
    fn two_colours_are_kept() {
        let encoder = Rgbcx::default();
        let (a, b) = ([255u8, 0, 0, 255], [0u8, 0, 255, 255]);
        let mut source = [a; 16];
        for pixel in source.iter_mut().skip(8) {
            *pixel = b;
        }
        let mut block = [0u8; 8];
        encoder.encode_bc1(10, &mut block, &source, false, false, None);
        let mut pixels = [[0u8; 4]; 16];
        unpack_bc1(&block, &mut pixels, true, Bc1ApproxMode::Ideal);
        assert_eq!(pixels, source);
    }

    #[test]
    fn bc4_keeps_eight_even_steps() {
        // 0, 36, ... 252 are the eight values of a block from 252 down to 0, exactly.
        let mut values = [0u8; 16];
        for (i, value) in values.iter_mut().enumerate() {
            *value = (i as u8 % 8) * 36;
        }
        let mut block = [0u8; 8];
        encode_bc4(&mut block, &values, 1);
        assert_eq!(&block[..2], &[252, 0]);
        let mut unpacked = [0u8; 16];
        unpack_bc4(&block, &mut unpacked, 1);
        assert_eq!(unpacked, values);

        let mut searched = [0u8; 8];
        assert_eq!(
            encode_bc4_hq(
                &mut searched,
                &values,
                1,
                BC4_DEFAULT_SEARCH_RAD,
                BC4_USE_ALL_MODES,
                None
            ),
            0
        );
        unpack_bc4(&searched, &mut unpacked, 1);
        assert_eq!(unpacked, values);
    }

    #[test]
    fn bc3_and_bc5_put_their_channels_where_they_belong() {
        let encoder = Rgbcx::default();
        let mut source = [[0u8; 4]; 16];
        for (i, pixel) in source.iter_mut().enumerate() {
            *pixel = [200, 100, 50, if i < 8 { 0 } else { 255 }];
        }
        let mut block = [0u8; 16];
        encoder.encode_bc3(5, &mut block, &source);
        let mut pixels = [[0u8; 4]; 16];
        assert!(unpack_bc3(&block, &mut pixels, Bc1ApproxMode::Ideal));
        assert!(pixels
            .iter()
            .enumerate()
            .all(|(i, pixel)| pixel[3] == source[i][3]));

        encode_bc5(&mut block, source.as_flattened(), 0, 3, 4);
        let mut two = [0u8; 64];
        unpack_bc5(&block, &mut two, 0, 1, 4);
        assert!((0..16).all(|i| two[i * 4] == 200 && two[i * 4 + 1] == source[i][3]));
    }
}
