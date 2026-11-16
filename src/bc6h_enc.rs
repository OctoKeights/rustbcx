//! BC6H blocks from HDR pixels, signed and unsigned: the port of `bc6h_enc.h` (Alexander
//! Samusev, <https://github.com/0xc0de/bc6h_enc>, MIT), which is the BC6H encoder of Microsoft's
//! DirectXTex (`BC6HBC7.cpp`) made into one file.
//!
//! It tries each of the fourteen modes in turn, and for the modes of two regions the eight
//! likeliest of the thirty-two shapes, moves the endpoints of each a step at a time for as
//! long as that helps, and keeps the block with the least error. That is thorough and slow:
//! [`crate::bc6h`] has the quick encoder, for unsigned blocks only. This one is the crate's
//! way to signed BC6H ([`encode_bc6hs`]).
//!
//! The port follows the original built with `BC6H_SSE_INTRINSICS`, which is the build that
//! does what DirectXTex does: floats become halves rounded to the nearest, ties to even, and
//! the error of a pixel is taken over red, green and blue. (Built without it, bc6h_enc rounds
//! halves another way, and takes the error over a fourth value as well that it never sets.)
//!
//! The decoder that comes with bc6h_enc is not ported: [`crate::bc6h`] unpacks both kinds.
//! A pixel's values have to be numbers; what is infinite or not a number has no place in a
//! block, and the original does not say what it makes of one.

use crate::bc6h::float_to_half;
use crate::bc6h_enc_tables::{field, DESC, FIX_UP, PARTITION_TABLE};

/// A pixel: red, green, blue and alpha as floats (`HDRColorA`). The alpha is not used.
pub type HdrColorA = [f32; 4];

const F16S_MASK: u16 = 0x8000; // f16 sign mask
const F16EM_MASK: u16 = 0x7fff; // f16 exp & mantissa mask
const F16MAX: i32 = 0x7bff; // MAXFLT bit pattern for XMHALF

const BC6H_NUM_PIXELS_PER_BLOCK: usize = 16;
const BC6H_MAX_REGIONS: usize = 2;
const BC6H_MAX_INDICES: usize = 16;
const BC6H_NUM_CHANNELS: usize = 3;
const BC6H_MAX_SHAPES: usize = 32;
const BC6H_WEIGHT_MAX: i32 = 64;
const BC6H_WEIGHT_SHIFT: u32 = 6;
const BC6H_WEIGHT_ROUND: i32 = 32;

const F_EPSILON: f32 = (0.25 / 64.0) * (0.25 / 64.0);
// (Written as the original has them.)
#[allow(clippy::eq_op)]
const P_C3: [f32; 3] = [2.0 / 2.0, 1.0 / 2.0, 0.0 / 2.0];
#[allow(clippy::eq_op)]
const P_D3: [f32; 3] = [0.0 / 2.0, 1.0 / 2.0, 2.0 / 2.0];
#[allow(clippy::eq_op)]
const P_C4: [f32; 4] = [3.0 / 3.0, 2.0 / 3.0, 1.0 / 3.0, 0.0 / 3.0];
#[allow(clippy::eq_op)]
const P_D4: [f32; 4] = [0.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 3.0 / 3.0];

const G_A_WEIGHTS3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const G_A_WEIGHTS4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Red, green and blue as the integers a block works in: `INTColor`.
type IntColor = [i32; 3];

/// `INTEndPntPair`
#[derive(Clone, Copy, Default)]
struct IntEndPntPair {
    a: IntColor,
    b: IntColor,
}

/// `INTColor::F16ToINT`
fn f16_to_int(f: u16, signed: bool) -> i32 {
    let mut input = f;
    let out;
    if signed {
        let s = input & F16S_MASK;
        input &= F16EM_MASK;
        let magnitude = if i32::from(input) > F16MAX {
            F16MAX
        } else {
            i32::from(input)
        };
        out = if s != 0 { -magnitude } else { magnitude };
    } else if input & F16S_MASK != 0 {
        out = 0;
    } else {
        out = i32::from(input);
    }
    out
}

/// `INTColor::Set`: a pixel's floats as halves (rounded to the nearest, ties to even: what
/// `XMStoreHalf4` does with F16C), and the halves as integers.
fn int_color_set(c: &[f32; 3], signed: bool) -> IntColor {
    [
        f16_to_int(float_to_half(c[0]), signed),
        f16_to_int(float_to_half(c[1]), signed),
        f16_to_int(float_to_half(c[2]), signed),
    ]
}

/// `INTColor::Clamp`
fn int_color_clamp(c: &mut IntColor, min: i32, max: i32) {
    for value in c {
        *value = max.min(min.max(*value));
    }
}

/// `INTColor::SignExtend`
fn int_color_sign_extend(c: &mut IntColor, prec: &[u8; 3]) {
    for (value, &nb) in c.iter_mut().zip(prec) {
        let nb = i32::from(nb);
        *value |= if *value & (1 << (nb - 1)) != 0 {
            !0 ^ ((1 << nb) - 1)
        } else {
            0
        };
    }
}

/// `ModeInfo`: a mode's number in a block, whether it has two regions, whether its endpoints
/// after the first are held as differences from the first, the bits of an index, and the bits
/// of each endpoint's red, green and blue.
struct ModeInfo {
    mode: u8,
    partitions: u8,
    transformed: bool,
    index_prec: u8,
    rgba_prec: [[[u8; 3]; 2]; BC6H_MAX_REGIONS],
}

const fn info(
    mode: u8,
    partitions: u8,
    transformed: bool,
    index_prec: u8,
    first: [u8; 3],
    rest: [u8; 3],
) -> ModeInfo {
    // (A mode of one region has no second pair of endpoints, and none of their bits.)
    let second = if partitions != 0 { rest } else { [0, 0, 0] };
    ModeInfo {
        mode,
        partitions,
        transformed,
        index_prec,
        rgba_prec: [[first, rest], [second, second]],
    }
}

// Mode, Partitions, Transformed, IndexPrec, RGBAPrec
#[rustfmt::skip]
const MS_A_INFO: [ModeInfo; 14] = [
    info(0x00, 1, true, 3, [10, 10, 10], [5, 5, 5]),    // Mode 1
    info(0x01, 1, true, 3, [7, 7, 7], [6, 6, 6]),       // Mode 2
    info(0x02, 1, true, 3, [11, 11, 11], [5, 4, 4]),    // Mode 3
    info(0x06, 1, true, 3, [11, 11, 11], [4, 5, 4]),    // Mode 4
    info(0x0a, 1, true, 3, [11, 11, 11], [4, 4, 5]),    // Mode 5
    info(0x0e, 1, true, 3, [9, 9, 9], [5, 5, 5]),       // Mode 6
    info(0x12, 1, true, 3, [8, 8, 8], [6, 5, 5]),       // Mode 7
    info(0x16, 1, true, 3, [8, 8, 8], [5, 6, 5]),       // Mode 8
    info(0x1a, 1, true, 3, [8, 8, 8], [5, 5, 6]),       // Mode 9
    info(0x1e, 1, false, 3, [6, 6, 6], [6, 6, 6]),      // Mode 10
    info(0x03, 0, false, 4, [10, 10, 10], [10, 10, 10]), // Mode 11
    info(0x07, 0, true, 4, [11, 11, 11], [9, 9, 9]),    // Mode 12
    info(0x0b, 0, true, 4, [12, 12, 12], [8, 8, 8]),    // Mode 13
    info(0x0f, 0, true, 4, [16, 16, 16], [4, 4, 4]),    // Mode 14
];

/// `CBits::SetBit`
fn set_bit(bits: &mut [u8; 16], start_bit: &mut usize, value: u8) {
    debug_assert!(*start_bit < 128 && value < 2);
    let index = *start_bit >> 3;
    let base = *start_bit - (index << 3);
    bits[index] &= !(1 << base);
    bits[index] |= value << base;
    *start_bit += 1;
}

/// `CBits::SetBits`
fn set_bits(bits: &mut [u8; 16], start_bit: &mut usize, num_bits: usize, value: u8) {
    if num_bits == 0 {
        return;
    }
    debug_assert!(*start_bit + num_bits <= 128 && num_bits <= 8);
    debug_assert!(u32::from(value) < (1 << num_bits));
    let index = *start_bit >> 3;
    let base = *start_bit - (index << 3);
    if base + num_bits > 8 {
        let first_index_bits = 8 - base;
        let next_index_bits = num_bits - first_index_bits;
        bits[index] &= !((((1u32 << first_index_bits) - 1) << base) as u8);
        bits[index] |= ((u32::from(value)) << base) as u8;
        bits[index + 1] &= !(((1u32 << next_index_bits) - 1) as u8);
        bits[index + 1] |= value >> first_index_bits;
    } else {
        bits[index] &= !((((1u32 << num_bits) - 1) << base) as u8);
        bits[index] |= value << base;
    }
    *start_bit += num_bits;
}

/// `EncodeParams`
struct EncodeParams<'a> {
    best_err: f32,
    signed: bool,
    /// The place of the mode being tried in [`MS_A_INFO`].
    mode: usize,
    shape: usize,
    hdr_pixels: &'a [HdrColorA; 16],
    unq_end_pts: [[IntEndPntPair; BC6H_MAX_REGIONS]; BC6H_MAX_SHAPES],
    i_pixels: [IntColor; BC6H_NUM_PIXELS_PER_BLOCK],
}

impl<'a> EncodeParams<'a> {
    fn new(original: &'a [HdrColorA; 16], signed: bool) -> EncodeParams<'a> {
        let mut i_pixels = [[0i32; 3]; BC6H_NUM_PIXELS_PER_BLOCK];
        for (pixel, color) in i_pixels.iter_mut().zip(original) {
            *pixel = int_color_set(&[color[0], color[1], color[2]], signed);
        }
        EncodeParams {
            best_err: f32::MAX,
            signed,
            mode: 0,
            shape: 0,
            hdr_pixels: original,
            unq_end_pts: [[IntEndPntPair::default(); BC6H_MAX_REGIONS]; BC6H_MAX_SHAPES],
            i_pixels,
        }
    }

    fn info(&self) -> &'static ModeInfo {
        &MS_A_INFO[self.mode]
    }
}

//-------------------------------------------------------------------------------------
// Helper functions
//-------------------------------------------------------------------------------------
fn is_fix_up_offset(partitions: usize, shape: usize, offset: usize) -> bool {
    debug_assert!(partitions < 3 && shape < BC6H_MAX_SHAPES && offset < 16);
    for p in 0..=partitions {
        if offset == usize::from(FIX_UP[partitions][shape][p]) {
            return true;
        }
    }
    false
}

fn transform_forward(end_pts: &mut [IntEndPntPair; BC6H_MAX_REGIONS]) {
    for c in 0..3 {
        end_pts[0].b[c] -= end_pts[0].a[c];
        end_pts[1].a[c] -= end_pts[0].a[c];
        end_pts[1].b[c] -= end_pts[0].a[c];
    }
}

fn transform_inverse(
    end_pts: &mut [IntEndPntPair; BC6H_MAX_REGIONS],
    prec: &[u8; 3],
    signed: bool,
) {
    for c in 0..3 {
        let wrap_mask = (1 << prec[c]) - 1;
        end_pts[0].b[c] += end_pts[0].a[c];
        end_pts[0].b[c] &= wrap_mask;
        end_pts[1].a[c] += end_pts[0].a[c];
        end_pts[1].a[c] &= wrap_mask;
        end_pts[1].b[c] += end_pts[0].a[c];
        end_pts[1].b[c] &= wrap_mask;
    }
    if signed {
        int_color_sign_extend(&mut end_pts[0].b, prec);
        int_color_sign_extend(&mut end_pts[1].a, prec);
        int_color_sign_extend(&mut end_pts[1].b, prec);
    }
}

fn norm(a: &IntColor, b: &IntColor) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    dr * dr + dg * dg + db * db
}

// return # of bits needed to store n. handle signed or unsigned cases properly
fn n_bits(mut n: i32, is_signed: bool) -> i32 {
    let mut nb;
    if n == 0 {
        0 // no bits needed for 0, signed or not
    } else if n > 0 {
        nb = 0;
        while n != 0 {
            nb += 1;
            n >>= 1;
        }
        nb + i32::from(is_signed)
    } else {
        nb = 0;
        while n < -1 {
            nb += 1;
            n >>= 1;
        }
        nb + 1
    }
}

/// `OptimizeRGB`: the two ends of the line through the pixels named in `index` that a palette
/// of `steps` colours along it fits best.
fn optimize_rgb(points: &[HdrColorA; 16], steps: u32, index: &[usize]) -> ([f32; 3], [f32; 3]) {
    let (p_c, p_d): (&[f32], &[f32]) = if steps == 3 {
        (&P_C3, &P_D3)
    } else {
        (&P_C4, &P_D4)
    };

    // Find Min and Max points, as starting point
    let mut x = [f32::MAX; 3];
    let mut y = [-f32::MAX; 3];

    for &i in index {
        for c in 0..3 {
            if points[i][c] < x[c] {
                x[c] = points[i][c];
            }
        }
        for c in 0..3 {
            if points[i][c] > y[c] {
                y[c] = points[i][c];
            }
        }
    }

    // Diagonal axis
    let ab = [y[0] - x[0], y[1] - x[1], y[2] - x[2]];

    let f_ab = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];

    // Single color block.. no need to root-find
    if f_ab < f32::MIN_POSITIVE {
        return (x, y);
    }

    // Try all four axis directions, to determine which diagonal best fits data
    let f_ab_inv = 1.0 / f_ab;

    let mut dir = [ab[0] * f_ab_inv, ab[1] * f_ab_inv, ab[2] * f_ab_inv];

    let mid = [
        (x[0] + y[0]) * 0.5,
        (x[1] + y[1]) * 0.5,
        (x[2] + y[2]) * 0.5,
    ];

    let mut f_dir = [0.0f32; 4];

    for &i in index {
        let pt = [
            (points[i][0] - mid[0]) * dir[0],
            (points[i][1] - mid[1]) * dir[1],
            (points[i][2] - mid[2]) * dir[2],
        ];

        let mut f;
        f = pt[0] + pt[1] + pt[2];
        f_dir[0] += f * f;
        f = pt[0] + pt[1] - pt[2];
        f_dir[1] += f * f;
        f = pt[0] - pt[1] + pt[2];
        f_dir[2] += f * f;
        f = pt[0] - pt[1] - pt[2];
        f_dir[3] += f * f;
    }

    let mut f_dir_max = f_dir[0];
    let mut i_dir_max = 0usize;

    for i_dir in 1..4 {
        if f_dir[i_dir] > f_dir_max {
            f_dir_max = f_dir[i_dir];
            i_dir_max = i_dir;
        }
    }

    if i_dir_max & 2 != 0 {
        std::mem::swap(&mut x[1], &mut y[1]);
    }
    if i_dir_max & 1 != 0 {
        std::mem::swap(&mut x[2], &mut y[2]);
    }

    // Two color block.. no need to root-find
    if f_ab < 1.0 / 4096.0 {
        return (x, y);
    }

    // Use Newton's Method to find local minima of sum-of-squares error.
    let f_steps = (steps - 1) as f32;

    for _ in 0..8 {
        // Calculate new steps
        let mut p_steps = [[0.0f32; 3]; 4];

        for i_step in 0..steps as usize {
            p_steps[i_step][0] = x[0] * p_c[i_step] + y[0] * p_d[i_step];
            p_steps[i_step][1] = x[1] * p_c[i_step] + y[1] * p_d[i_step];
            p_steps[i_step][2] = x[2] * p_c[i_step] + y[2] * p_d[i_step];
        }

        // Calculate color direction
        dir[0] = y[0] - x[0];
        dir[1] = y[1] - x[1];
        dir[2] = y[2] - x[2];

        let f_len = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];

        if f_len < (1.0 / 4096.0) {
            break;
        }

        let f_scale = f_steps / f_len;

        dir[0] *= f_scale;
        dir[1] *= f_scale;
        dir[2] *= f_scale;

        // Evaluate function, and derivatives
        let (mut d2x, mut d2y) = (0.0f32, 0.0f32);
        let (mut dx, mut dy) = ([0.0f32; 3], [0.0f32; 3]);

        for &i in index {
            let f_dot = (points[i][0] - x[0]) * dir[0]
                + (points[i][1] - x[1]) * dir[1]
                + (points[i][2] - x[2]) * dir[2];

            let i_step = if f_dot <= 0.0 {
                0
            } else if f_dot >= f_steps {
                steps as usize - 1
            } else {
                (f_dot + 0.5) as u32 as usize
            };

            let diff = [
                p_steps[i_step][0] - points[i][0],
                p_steps[i_step][1] - points[i][1],
                p_steps[i_step][2] - points[i][2],
            ];

            let f_c = p_c[i_step] * (1.0 / 8.0);
            let f_d = p_d[i_step] * (1.0 / 8.0);

            d2x += f_c * p_c[i_step];
            dx[0] += f_c * diff[0];
            dx[1] += f_c * diff[1];
            dx[2] += f_c * diff[2];

            d2y += f_d * p_d[i_step];
            dy[0] += f_d * diff[0];
            dy[1] += f_d * diff[1];
            dy[2] += f_d * diff[2];
        }

        // Move endpoints
        if d2x > 0.0 {
            let f = -1.0 / d2x;

            x[0] += dx[0] * f;
            x[1] += dx[1] * f;
            x[2] += dx[2] * f;
        }

        if d2y > 0.0 {
            let f = -1.0 / d2y;

            y[0] += dy[0] * f;
            y[1] += dy[1] * f;
            y[2] += dy[2] * f;
        }

        if (dx[0] * dx[0] < F_EPSILON)
            && (dx[1] * dx[1] < F_EPSILON)
            && (dx[2] * dx[2] < F_EPSILON)
            && (dy[0] * dy[0] < F_EPSILON)
            && (dy[1] * dy[1] < F_EPSILON)
            && (dy[2] * dy[2] < F_EPSILON)
        {
            break;
        }
    }

    (x, y)
}

/// `D3DX_BC6H::Encode`
fn encode(block: &mut [u8; 16], signed: bool, pixels: &[HdrColorA; 16]) {
    let mut ep = EncodeParams::new(pixels, signed);

    ep.mode = 0;
    while ep.mode < MS_A_INFO.len() && ep.best_err > 0.0 {
        let shapes: usize = if ep.info().partitions != 0 { 32 } else { 1 };
        // Number of rough cases to look at. reasonable values of this are 1, uShapes/4, and uShapes
        // uShapes/4 gets nearly all the cases; you can increase that a bit
        // (say by 3 or 4) if you really want to squeeze the last bit out
        let items = 1.max(shapes >> 2);
        let mut rough_mse_of = [0.0f32; BC6H_MAX_SHAPES];
        let mut shape_of = [0usize; BC6H_MAX_SHAPES];

        // pick the best uItems shapes and refine these.
        for shape in 0..shapes {
            ep.shape = shape;
            rough_mse_of[shape] = rough_mse(&mut ep);
            shape_of[shape] = shape;
        }

        // Bubble up the first uItems items
        for i in 0..items {
            for j in i + 1..shapes {
                if rough_mse_of[i] > rough_mse_of[j] {
                    rough_mse_of.swap(i, j);
                    shape_of.swap(i, j);
                }
            }
        }

        let mut i = 0;
        while i < items && ep.best_err > 0.0 {
            ep.shape = shape_of[i];
            refine(block, &mut ep);
            i += 1;
        }

        ep.mode += 1;
    }
}

/// `D3DX_BC6H::Quantize`
fn quantize(mut value: i32, prec: i32, signed: bool) -> i32 {
    debug_assert!(prec > 1); // didn't bother to make it work for 1
    let mut q;
    if signed {
        debug_assert!((-F16MAX..=F16MAX).contains(&value));
        let mut s = false;
        if value < 0 {
            s = true;
            value = -value;
        }
        q = if prec >= 16 {
            value
        } else {
            (value << (prec - 1)) / (F16MAX + 1)
        };
        if s {
            q = -q;
        }
        debug_assert!(q > -(1 << (prec - 1)) && q < (1 << (prec - 1)));
    } else {
        debug_assert!((0..=F16MAX).contains(&value));
        q = if prec >= 15 {
            value
        } else {
            (value << prec) / (F16MAX + 1)
        };
        debug_assert!(q >= 0 && q < (1 << prec));
    }

    q
}

/// `D3DX_BC6H::Unquantize`
fn unquantize(mut comp: i32, bits_per_comp: u8, signed: bool) -> i32 {
    let bits_per_comp = i32::from(bits_per_comp);
    let mut unq;
    if signed {
        if bits_per_comp >= 16 {
            unq = comp;
        } else {
            let mut s = false;
            if comp < 0 {
                s = true;
                comp = -comp;
            }

            if comp == 0 {
                unq = 0;
            } else if comp >= ((1 << (bits_per_comp - 1)) - 1) {
                unq = 0x7FFF;
            } else {
                unq = ((comp << 15) + 0x4000) >> (bits_per_comp - 1);
            }

            if s {
                unq = -unq;
            }
        }
    } else if bits_per_comp >= 15 {
        unq = comp;
    } else if comp == 0 {
        unq = 0;
    } else if comp == ((1 << bits_per_comp) - 1) {
        unq = 0xFFFF;
    } else {
        unq = ((comp << 16) + 0x8000) >> bits_per_comp;
    }

    unq
}

/// `D3DX_BC6H::FinishUnquantize`
fn finish_unquantize(comp: i32, signed: bool) -> i32 {
    if signed {
        if comp < 0 {
            -(((-comp) * 31) >> 5)
        } else {
            (comp * 31) >> 5 // scale the magnitude by 31/32
        }
    } else {
        (comp * 31) >> 6 // scale the magnitude by 31/64
    }
}

/// `D3DX_BC6H::EndPointsFit`
fn end_points_fit(ep: &EncodeParams, end_pts: &[IntEndPntPair; BC6H_MAX_REGIONS]) -> bool {
    let info = ep.info();
    let transformed = info.transformed;
    let is_signed = ep.signed;
    let prec0 = &info.rgba_prec[0][0];
    let prec1 = &info.rgba_prec[0][1];
    let prec2 = &info.rgba_prec[1][0];
    let prec3 = &info.rgba_prec[1][1];

    for c in 0..3 {
        if n_bits(end_pts[0].a[c], is_signed) > i32::from(prec0[c])
            || n_bits(end_pts[0].b[c], transformed || is_signed) > i32::from(prec1[c])
        {
            return false;
        }
    }

    if info.partitions != 0 {
        for c in 0..3 {
            if n_bits(end_pts[1].a[c], transformed || is_signed) > i32::from(prec2[c])
                || n_bits(end_pts[1].b[c], transformed || is_signed) > i32::from(prec3[c])
            {
                return false;
            }
        }
    }

    true
}

/// The weights of a mode's indices.
fn weights_of(index_prec: u8) -> &'static [i32] {
    if index_prec == 3 {
        &G_A_WEIGHTS3
    } else {
        &G_A_WEIGHTS4
    }
}

/// `D3DX_BC6H::GeneratePaletteQuantized`
fn generate_palette_quantized(
    ep: &EncodeParams,
    end_pts: &IntEndPntPair,
    palette: &mut [IntColor; BC6H_MAX_INDICES],
) {
    let info = ep.info();
    let num_indices = 1usize << info.index_prec;
    let prec = &info.rgba_prec[0][0];

    // scale endpoints
    let mut unq_end_pts = IntEndPntPair::default();
    for c in 0..3 {
        unq_end_pts.a[c] = unquantize(end_pts.a[c], prec[c], ep.signed);
    }
    for c in 0..3 {
        unq_end_pts.b[c] = unquantize(end_pts.b[c], prec[c], ep.signed);
    }

    // interpolate
    let weights = weights_of(info.index_prec);

    for i in 0..num_indices {
        for c in 0..3 {
            palette[i][c] = finish_unquantize(
                (unq_end_pts.a[c] * (BC6H_WEIGHT_MAX - weights[i])
                    + unq_end_pts.b[c] * weights[i]
                    + BC6H_WEIGHT_ROUND)
                    >> BC6H_WEIGHT_SHIFT,
                ep.signed,
            );
        }
    }
}

// given a collection of colors and quantized endpoints, generate
// a palette, choose best entries, and return a single toterr
/// `D3DX_BC6H::MapColorsQuantized`
fn map_colors_quantized(ep: &EncodeParams, colors: &[IntColor], end_pts: &IntEndPntPair) -> f32 {
    let num_indices = 1usize << ep.info().index_prec;
    let mut palette = [[0i32; 3]; BC6H_MAX_INDICES];
    generate_palette_quantized(ep, end_pts, &mut palette);

    let mut tot_err = 0.0f32;
    for color in colors {
        // Compute ErrorMetricRGB
        let mut best_err = norm(color, &palette[0]);

        let mut j = 1;
        while j < num_indices && best_err > 0.0 {
            // Compute ErrorMetricRGB
            let err = norm(color, &palette[j]);
            if err > best_err {
                break; // error increased, so we're done searching
            }
            if err < best_err {
                best_err = err;
            }
            j += 1;
        }
        tot_err += best_err;
    }
    tot_err
}

/// `D3DX_BC6H::PerturbOne`
fn perturb_one(
    ep: &EncodeParams,
    colors: &[IntColor],
    ch: usize,
    old_end_pts: &IntEndPntPair,
    new_end_pts: &mut IntEndPntPair,
    old_err: f32,
    do_b: i32,
) -> f32 {
    let prec = i32::from(ep.info().rgba_prec[0][0][ch]);
    let mut min_err = old_err;
    let mut beststep = 0;

    // copy real endpoints so we can perturb them
    *new_end_pts = *old_end_pts;
    let mut tmp_end_pts = *old_end_pts;

    // do a logarithmic search for the best error for this endpoint (which)
    let mut step = 1 << (prec - 1);
    while step != 0 {
        let mut improved = false;
        for sign in [-1, 1] {
            if do_b == 0 {
                tmp_end_pts.a[ch] = new_end_pts.a[ch] + sign * step;
                if tmp_end_pts.a[ch] < 0 || tmp_end_pts.a[ch] >= (1 << prec) {
                    continue;
                }
            } else {
                tmp_end_pts.b[ch] = new_end_pts.b[ch] + sign * step;
                if tmp_end_pts.b[ch] < 0 || tmp_end_pts.b[ch] >= (1 << prec) {
                    continue;
                }
            }

            let err = map_colors_quantized(ep, colors, &tmp_end_pts);

            if err < min_err {
                improved = true;
                min_err = err;
                beststep = sign * step;
            }
        }
        // if this was an improvement, move the endpoint and continue search from there
        if improved {
            if do_b == 0 {
                new_end_pts.a[ch] += beststep;
            } else {
                new_end_pts.b[ch] += beststep;
            }
        }
        step >>= 1;
    }
    min_err
}

/// `D3DX_BC6H::OptimizeOne`
fn optimize_one(
    ep: &EncodeParams,
    colors: &[IntColor],
    org_err: f32,
    org_end_pts: &IntEndPntPair,
    opt_end_pts: &mut IntEndPntPair,
) {
    let mut opt_err = org_err;
    opt_end_pts.a = org_end_pts.a;
    opt_end_pts.b = org_end_pts.b;

    let (mut new_a, mut new_b) = (IntEndPntPair::default(), IntEndPntPair::default());
    let mut new_end_pts = IntEndPntPair::default();
    let mut do_b;

    // now optimize each channel separately
    for ch in 0..BC6H_NUM_CHANNELS {
        // figure out which endpoint when perturbed gives the most improvement and start there
        // if we just alternate, we can easily end up in a local minima
        let current = *opt_end_pts;
        let err0 = perturb_one(ep, colors, ch, &current, &mut new_a, opt_err, 0); // perturb endpt A
        let err1 = perturb_one(ep, colors, ch, &current, &mut new_b, opt_err, 1); // perturb endpt B

        if err0 < err1 {
            if err0 >= opt_err {
                continue;
            }
            opt_end_pts.a[ch] = new_a.a[ch];
            opt_err = err0;
            do_b = 1; // do B next
        } else {
            if err1 >= opt_err {
                continue;
            }
            opt_end_pts.b[ch] = new_b.b[ch];
            opt_err = err1;
            do_b = 0; // do A next
        }

        // now alternate endpoints and keep trying until there is no improvement
        loop {
            let current = *opt_end_pts;
            let err = perturb_one(ep, colors, ch, &current, &mut new_end_pts, opt_err, do_b);
            if err >= opt_err {
                break;
            }
            if do_b == 0 {
                opt_end_pts.a[ch] = new_end_pts.a[ch];
            } else {
                opt_end_pts.b[ch] = new_end_pts.b[ch];
            }
            opt_err = err;
            do_b = 1 - do_b; // now move the other endpoint
        }
    }
}

/// `D3DX_BC6H::OptimizeEndPoints`
fn optimize_end_points(
    ep: &EncodeParams,
    org_err: &[f32; BC6H_MAX_REGIONS],
    org_end_pts: &[IntEndPntPair; BC6H_MAX_REGIONS],
    opt_end_pts: &mut [IntEndPntPair; BC6H_MAX_REGIONS],
) {
    let partitions = usize::from(ep.info().partitions);
    let mut pixels = [[0i32; 3]; BC6H_NUM_PIXELS_PER_BLOCK];

    for p in 0..=partitions {
        // collect the pixels in the region
        // (The table is taken by the region, not by the number of regions, as in the
        // original: the first region of two is given every pixel of the block.)
        let mut np = 0;
        for i in 0..BC6H_NUM_PIXELS_PER_BLOCK {
            if usize::from(PARTITION_TABLE[p][ep.shape][i]) == p {
                pixels[np] = ep.i_pixels[i];
                np += 1;
            }
        }

        optimize_one(
            ep,
            &pixels[..np],
            org_err[p],
            &org_end_pts[p],
            &mut opt_end_pts[p],
        );
    }
}

// Swap endpoints as needed to ensure that the indices at fix up have a 0 high-order bit
/// `D3DX_BC6H::SwapIndices`
fn swap_indices(
    ep: &EncodeParams,
    end_pts: &mut [IntEndPntPair; BC6H_MAX_REGIONS],
    indices: &mut [usize; BC6H_NUM_PIXELS_PER_BLOCK],
) {
    let partitions = usize::from(ep.info().partitions);
    let num_indices = 1usize << ep.info().index_prec;
    let high_index_bit = num_indices >> 1;

    for p in 0..=partitions {
        let i = usize::from(FIX_UP[partitions][ep.shape][p]);
        debug_assert!(usize::from(PARTITION_TABLE[partitions][ep.shape][i]) == p);
        if indices[i] & high_index_bit != 0 {
            // high bit is set, swap the aEndPts and indices for this region
            std::mem::swap(&mut end_pts[p].a, &mut end_pts[p].b);

            for j in 0..BC6H_NUM_PIXELS_PER_BLOCK {
                if usize::from(PARTITION_TABLE[partitions][ep.shape][j]) == p {
                    indices[j] = num_indices - 1 - indices[j];
                }
            }
        }
    }
}

// assign indices given a tile, shape, and quantized endpoints, return toterr for each region
/// `D3DX_BC6H::AssignIndices`
fn assign_indices(
    ep: &EncodeParams,
    end_pts: &[IntEndPntPair; BC6H_MAX_REGIONS],
    indices: &mut [usize; BC6H_NUM_PIXELS_PER_BLOCK],
    tot_err: &mut [f32; BC6H_MAX_REGIONS],
) {
    let partitions = usize::from(ep.info().partitions);
    let num_indices = 1usize << ep.info().index_prec;

    // build list of possibles
    let mut palette = [[[0i32; 3]; BC6H_MAX_INDICES]; BC6H_MAX_REGIONS];

    for p in 0..=partitions {
        generate_palette_quantized(ep, &end_pts[p], &mut palette[p]);
        tot_err[p] = 0.0;
    }

    for i in 0..BC6H_NUM_PIXELS_PER_BLOCK {
        let region = usize::from(PARTITION_TABLE[partitions][ep.shape][i]);
        let mut best_err = norm(&ep.i_pixels[i], &palette[region][0]);
        indices[i] = 0;

        let mut j = 1;
        while j < num_indices && best_err > 0.0 {
            let err = norm(&ep.i_pixels[i], &palette[region][j]);
            if err > best_err {
                break; // error increased, so we're done searching
            }
            if err < best_err {
                best_err = err;
                indices[i] = j;
            }
            j += 1;
        }
        tot_err[region] += best_err;
    }
}

/// `D3DX_BC6H::QuantizeEndPts`
fn quantize_end_pts(ep: &EncodeParams, qnt_end_pts: &mut [IntEndPntPair; BC6H_MAX_REGIONS]) {
    let unq_end_pts = &ep.unq_end_pts[ep.shape];
    let prec = &ep.info().rgba_prec[0][0];
    let partitions = usize::from(ep.info().partitions);

    for p in 0..=partitions {
        for c in 0..3 {
            qnt_end_pts[p].a[c] = quantize(unq_end_pts[p].a[c], i32::from(prec[c]), ep.signed);
        }
        for c in 0..3 {
            qnt_end_pts[p].b[c] = quantize(unq_end_pts[p].b[c], i32::from(prec[c]), ep.signed);
        }
    }
}

/// `D3DX_BC6H::EmitBlock`
fn emit_block(
    block: &mut [u8; 16],
    ep: &EncodeParams,
    end_pts: &[IntEndPntPair; BC6H_MAX_REGIONS],
    indices: &[usize; BC6H_NUM_PIXELS_PER_BLOCK],
) {
    let info = ep.info();
    let real_mode = i32::from(info.mode);
    let partitions = usize::from(info.partitions);
    let index_prec = usize::from(info.index_prec);
    let header_bits = if partitions > 0 { 82 } else { 65 };
    let desc = &DESC[ep.mode];
    let mut start_bit = 0usize;

    while start_bit < header_bits {
        let [what, bit] = desc[start_bit];
        let value = match what {
            field::M => real_mode,
            field::D => ep.shape as i32,
            field::RW => end_pts[0].a[0],
            field::RX => end_pts[0].b[0],
            field::RY => end_pts[1].a[0],
            field::RZ => end_pts[1].b[0],
            field::GW => end_pts[0].a[1],
            field::GX => end_pts[0].b[1],
            field::GY => end_pts[1].a[1],
            field::GZ => end_pts[1].b[1],
            field::BW => end_pts[0].a[2],
            field::BX => end_pts[0].b[2],
            field::BY => end_pts[1].a[2],
            field::BZ => end_pts[1].b[2],
            _ => unreachable!("a bit of a header that holds nothing"),
        };
        set_bit(block, &mut start_bit, ((value >> bit) & 1) as u8);
    }

    for i in 0..BC6H_NUM_PIXELS_PER_BLOCK {
        if is_fix_up_offset(partitions, ep.shape, i) {
            set_bits(block, &mut start_bit, index_prec - 1, indices[i] as u8);
        } else {
            set_bits(block, &mut start_bit, index_prec, indices[i] as u8);
        }
    }
    debug_assert!(start_bit == 128);
}

/// `D3DX_BC6H::Refine`
fn refine(block: &mut [u8; 16], ep: &mut EncodeParams) {
    let partitions = usize::from(ep.info().partitions);

    let transformed = ep.info().transformed;
    let (mut org_err, mut opt_err) = ([0.0f32; BC6H_MAX_REGIONS], [0.0f32; BC6H_MAX_REGIONS]);
    let (mut org_end_pts, mut opt_end_pts) = (
        [IntEndPntPair::default(); BC6H_MAX_REGIONS],
        [IntEndPntPair::default(); BC6H_MAX_REGIONS],
    );
    let (mut org_idx, mut opt_idx) = (
        [0usize; BC6H_NUM_PIXELS_PER_BLOCK],
        [0usize; BC6H_NUM_PIXELS_PER_BLOCK],
    );

    quantize_end_pts(ep, &mut org_end_pts);
    assign_indices(ep, &org_end_pts, &mut org_idx, &mut org_err);
    swap_indices(ep, &mut org_end_pts, &mut org_idx);

    if transformed {
        transform_forward(&mut org_end_pts);
    }
    if end_points_fit(ep, &org_end_pts) {
        if transformed {
            transform_inverse(&mut org_end_pts, &ep.info().rgba_prec[0][0], ep.signed);
        }
        optimize_end_points(ep, &org_err, &org_end_pts, &mut opt_end_pts);
        assign_indices(ep, &opt_end_pts, &mut opt_idx, &mut opt_err);
        swap_indices(ep, &mut opt_end_pts, &mut opt_idx);

        let (mut org_tot_err, mut opt_tot_err) = (0.0f32, 0.0f32);
        for p in 0..=partitions {
            org_tot_err += org_err[p];
            opt_tot_err += opt_err[p];
        }

        if transformed {
            transform_forward(&mut opt_end_pts);
        }
        if end_points_fit(ep, &opt_end_pts)
            && opt_tot_err < org_tot_err
            && opt_tot_err < ep.best_err
        {
            ep.best_err = opt_tot_err;
            emit_block(block, ep, &opt_end_pts, &opt_idx);
        } else if org_tot_err < ep.best_err {
            // either it stopped fitting when we optimized it, or there was no improvement
            // so go back to the unoptimized endpoints which we know will fit
            if transformed {
                transform_forward(&mut org_end_pts);
            }
            ep.best_err = org_tot_err;
            emit_block(block, ep, &org_end_pts, &org_idx);
        }
    }
}

/// `D3DX_BC6H::GeneratePaletteUnquantized`
fn generate_palette_unquantized(
    ep: &EncodeParams,
    region: usize,
    palette: &mut [IntColor; BC6H_MAX_INDICES],
) {
    let end_pts = &ep.unq_end_pts[ep.shape][region];
    let index_prec = ep.info().index_prec;
    let num_indices = 1usize << index_prec;

    let weights = weights_of(index_prec);

    for i in 0..num_indices {
        for c in 0..3 {
            palette[i][c] = (end_pts.a[c] * (BC6H_WEIGHT_MAX - weights[i])
                + end_pts.b[c] * weights[i]
                + BC6H_WEIGHT_ROUND)
                >> BC6H_WEIGHT_SHIFT;
        }
    }
}

/// `D3DX_BC6H::MapColors`
fn map_colors(ep: &EncodeParams, region: usize, index: &[usize]) -> f32 {
    let num_indices = 1usize << ep.info().index_prec;
    let mut palette = [[0i32; 3]; BC6H_MAX_INDICES];
    generate_palette_unquantized(ep, region, &mut palette);

    let mut total_err = 0.0f32;
    for &i in index {
        let mut best_err = norm(&ep.i_pixels[i], &palette[0]);
        let mut j = 1;
        while j < num_indices && best_err > 0.0 {
            let err = norm(&ep.i_pixels[i], &palette[j]);
            if err > best_err {
                break; // error increased, so we're done searching
            }
            if err < best_err {
                best_err = err;
            }
            j += 1;
        }
        total_err += best_err;
    }

    total_err
}

/// `D3DX_BC6H::RoughMSE`
fn rough_mse(ep: &mut EncodeParams) -> f32 {
    let partitions = usize::from(ep.info().partitions);

    let mut pix_idx = [0usize; BC6H_NUM_PIXELS_PER_BLOCK];

    let mut error = 0.0f32;
    for p in 0..=partitions {
        let mut np = 0;
        for i in 0..BC6H_NUM_PIXELS_PER_BLOCK {
            if usize::from(PARTITION_TABLE[partitions][ep.shape][i]) == p {
                pix_idx[np] = i;
                np += 1;
            }
        }

        // handle simple cases
        debug_assert!(np > 0);
        if np == 1 {
            ep.unq_end_pts[ep.shape][p].a = ep.i_pixels[pix_idx[0]];
            ep.unq_end_pts[ep.shape][p].b = ep.i_pixels[pix_idx[0]];
            continue;
        } else if np == 2 {
            ep.unq_end_pts[ep.shape][p].a = ep.i_pixels[pix_idx[0]];
            ep.unq_end_pts[ep.shape][p].b = ep.i_pixels[pix_idx[1]];
            continue;
        }

        let (ep_a, ep_b) = optimize_rgb(ep.hdr_pixels, 4, &pix_idx[..np]);
        let (mut a, mut b) = (
            int_color_set(&ep_a, ep.signed),
            int_color_set(&ep_b, ep.signed),
        );
        if ep.signed {
            int_color_clamp(&mut a, -F16MAX, F16MAX);
            int_color_clamp(&mut b, -F16MAX, F16MAX);
        } else {
            int_color_clamp(&mut a, 0, F16MAX);
            int_color_clamp(&mut b, 0, F16MAX);
        }
        ep.unq_end_pts[ep.shape][p] = IntEndPntPair { a, b };

        error += map_colors(ep, p, &pix_idx[..np]);
    }

    error
}

//=====================================================================================
// Entry points
//=====================================================================================

/// Packs sixteen pixels, in rows from the top, into an unsigned BC6H block (`EncodeBC6HU`).
/// A value below zero is packed as zero, and one too large for a half as the largest half.
pub fn encode_bc6hu(dst: &mut [u8; 16], pixels: &[HdrColorA; 16]) {
    encode(dst, false, pixels);
}

/// Packs sixteen pixels, in rows from the top, into a signed BC6H block (`EncodeBC6HS`). A
/// value too large for a half, either way, is packed as the largest half of its sign.
pub fn encode_bc6hs(dst: &mut [u8; 16], pixels: &[HdrColorA; 16]) {
    encode(dst, true, pixels);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bc6h::{half_to_float, unpack_bc6h_half};

    fn unpack(block: &[u8; 16], signed: bool) -> Vec<[f32; 3]> {
        unpack_bc6h_half(block, signed)
            .iter()
            .map(|pixel| {
                [
                    half_to_float(pixel[0]),
                    half_to_float(pixel[1]),
                    half_to_float(pixel[2]),
                ]
            })
            .collect()
    }

    #[test]
    fn ramps_of_both_signs_come_back_near_themselves() {
        let mut pixels = [[0.0f32; 4]; 16];
        for (i, pixel) in pixels.iter_mut().enumerate() {
            let t = i as f32 / 15.0;
            // (Gentle ramps: a block goes from one end to the other in steps of the halves'
            // bits, which is not a straight line in their values.)
            *pixel = [-2.0 + 0.3 * t, 1.0 - 0.1 * t, -0.25 + 0.05 * t, 1.0];
        }
        let mut block = [0u8; 16];
        encode_bc6hs(&mut block, &pixels);
        for (a, b) in pixels.iter().zip(unpack(&block, true)) {
            for c in 0..3 {
                assert!(
                    (a[c] - b[c]).abs() <= 0.04 + a[c].abs() * 0.04,
                    "{a:?} {b:?}"
                );
            }
        }
        // What was in the block before does not matter.
        let mut other = [0xA5u8; 16];
        encode_bc6hs(&mut other, &pixels);
        assert_eq!(block, other);
    }

    #[test]
    fn unsigned_blocks_take_what_is_below_zero_as_zero() {
        let mut pixels = [[0.0f32; 4]; 16];
        for (i, pixel) in pixels.iter_mut().enumerate() {
            let t = i as f32 / 15.0;
            *pixel = [0.5 + 3.0 * t, 2.0, if i % 2 == 0 { -1.0 } else { 0.0 }, 1.0];
        }
        let mut block = [0u8; 16];
        encode_bc6hu(&mut block, &pixels);
        for (a, b) in pixels.iter().zip(unpack(&block, false)) {
            assert!(
                (a[0] - b[0]).abs() <= 0.05 + a[0] * 0.04 && (a[1] - b[1]).abs() <= 0.1,
                "{a:?} {b:?}"
            );
            assert!(b[2].abs() <= 0.01, "{a:?} {b:?}");
        }
    }

    #[test]
    fn one_colour_packs_exactly_enough() {
        for value in [0.0f32, 1.0, -1.0, 1000.0, -65504.0, 1e9, -1e9] {
            let pixels = [[value, value * 0.5, -value, 1.0]; 16];
            let mut block = [0u8; 16];
            encode_bc6hs(&mut block, &pixels);
            let clamp = |v: f32| v.clamp(-65504.0, 65504.0);
            for pixel in unpack(&block, true) {
                for (c, want) in [clamp(value), clamp(value * 0.5), clamp(-value)]
                    .into_iter()
                    .enumerate()
                {
                    assert!(
                        (pixel[c] - want).abs() <= want.abs() * 0.04 + 1e-3,
                        "{value}: {pixel:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn floats_of_any_bits_are_packed_to_blocks_of_a_mode() {
        // Infinities, not-a-numbers and the largest floats among them: nothing overflows, and
        // what comes out is a block the unpacker knows.
        let mut state = 0x1234_5678_9abc_def0u64;
        for i in 0..400 {
            let mut pixels = [[0.0f32; 4]; 16];
            for value in pixels.as_flattened_mut() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let bits = (state >> 32) as u32;
                *value = match i % 4 {
                    0 => f32::from_bits(bits),
                    1 => f32::from_bits(bits | 0x7f00_0000),
                    2 => [
                        f32::INFINITY,
                        f32::NEG_INFINITY,
                        f32::NAN,
                        f32::MAX,
                        -f32::MAX,
                        0.0,
                        -0.0,
                        1.0,
                    ][(bits % 8) as usize],
                    _ => (bits % 2000) as f32 - 1000.0,
                };
            }
            for signed in [false, true] {
                let mut block = [0u8; 16];
                encode(&mut block, signed, &pixels);
                let mode = if block[0] & 2 != 0 {
                    block[0] & 31
                } else {
                    block[0] & 3
                };
                assert!(
                    MS_A_INFO.iter().any(|info| info.mode == mode),
                    "{i} {signed}: {block:?}"
                );
            }
        }
    }

    #[test]
    fn bits_go_where_they_are_put() {
        let mut bits = [0xffu8; 16];
        let mut at = 5;
        set_bits(&mut bits, &mut at, 6, 0b10_1010);
        assert_eq!((at, bits[0], bits[1]), (11, 0b0101_1111, 0b1111_1101));
        set_bit(&mut bits, &mut at, 0);
        assert_eq!((at, bits[1]), (12, 0b1111_0101));
        set_bits(&mut bits, &mut at, 0, 0);
        assert_eq!(at, 12);
        assert_eq!(
            (
                n_bits(0, true),
                n_bits(5, false),
                n_bits(5, true),
                n_bits(-1, true),
                n_bits(-5, true),
                n_bits(-8, true)
            ),
            (0, 3, 4, 1, 4, 4)
        );
    }
}
