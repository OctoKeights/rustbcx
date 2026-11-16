//! The arithmetic of DWAA and DWAB as OpenEXRCore does it on an x86-64 machine with AVX and
//! F16C (any from about 2011 on), which is the code it picks there: its AVX inverse DCT, its
//! F16C conversions of halves, and its SSE2 colour conversion and forward DCT. Each sum and
//! product is the instruction's: the order of the operands is kept, and a NaN comes out as
//! the instruction gives it (the first operand's, quieted, then the second's, or the default
//! NaN for an invalid operation).

/// A NaN with its quiet bit set, as an x86 instruction passes one on.
fn quiet(x: f32) -> f32 {
    f32::from_bits(x.to_bits() | 0x0040_0000)
}

/// The NaN x86 gives for an invalid operation (infinity less infinity, zero times infinity).
const DEFAULT_NAN: u32 = 0xffc0_0000;

fn nan_or(a: f32, b: f32, r: f32) -> f32 {
    if a.is_nan() {
        quiet(a)
    } else if b.is_nan() {
        quiet(b)
    } else if r.is_nan() {
        f32::from_bits(DEFAULT_NAN)
    } else {
        r
    }
}

/// `addps`/`addss`, `a` its first source.
pub(super) fn add(a: f32, b: f32) -> f32 {
    nan_or(a, b, a + b)
}

/// `subps`/`subss`.
pub(super) fn sub(a: f32, b: f32) -> f32 {
    nan_or(a, b, a - b)
}

/// `mulps`/`mulss`, `a` its first source.
pub(super) fn mul(a: f32, b: f32) -> f32 {
    nan_or(a, b, a * b)
}

// ---------------------------------------------------------------------------------------
// Halves.

/// `half_to_float` of OpenEXRCore (Imath's): exact, a NaN keeping its bits.
pub(super) fn half_to_float(h: u16) -> f32 {
    let h = u32::from(h);
    let sign = (h >> 15) << 31;
    let exp = (h >> 10) & 0x1f;
    let mant = h & 0x3ff;
    let bits = if exp == 0x1f {
        sign | 0x7f80_0000 | (mant << 13)
    } else if exp != 0 {
        sign | ((exp + 112) << 23) | (mant << 13)
    } else if mant != 0 {
        // A denormal: shift the mantissa up to its leading one.
        let lead = 31 - mant.leading_zeros(); // 0..=9
        let shift = 10 - lead;
        sign | ((113 - shift) << 23) | (((mant << shift) & 0x3ff) << 13)
    } else {
        sign
    };
    f32::from_bits(bits)
}

/// `float_to_half` of OpenEXRCore (Imath's bit shifting): rounded to nearest, ties to even, a
/// NaN keeping the top of its payload (and a bit of it, if none of that is set).
pub(super) fn float_to_half(f: f32) -> u16 {
    let bits = f.to_bits();
    let mut ui = bits & !0x8000_0000;
    let mut ret = (bits >> 16) & 0x8000;

    if ui >= 0x3880_0000 {
        if ui >= 0x7f80_0000 {
            ret |= 0x7c00;
            if ui == 0x7f80_0000 {
                return ret as u16;
            }
            let m = (ui & 0x7f_ffff) >> 13;
            return (ret | m | u32::from(m == 0)) as u16;
        }
        if ui > 0x477f_efff {
            return (ret | 0x7c00) as u16;
        }
        ui -= 0x3800_0000;
        ui = (ui + 0x0000_0fff + ((ui >> 13) & 1)) >> 13;
        return (ret | ui) as u16;
    }

    // zero or flush to 0
    if ui < 0x3300_0001 {
        return ret as u16;
    }

    // produce a denormalized half
    let e = ui >> 23;
    let shift = 0x7e - e;
    let m = 0x80_0000 | (ui & 0x7f_ffff);
    let r = m << (32 - shift);
    ret |= m >> shift;
    if r > 0x8000_0000 || (r == 0x8000_0000 && (ret & 1) != 0) {
        ret += 1;
    }
    ret as u16
}

/// `vcvtph2ps`: exact, a NaN quieted.
pub(super) fn f16c_to_float(h: u16) -> f32 {
    let f = half_to_float(h);
    if f.is_nan() {
        quiet(f)
    } else {
        f
    }
}

/// `vcvtps2ph` with rounding to nearest, ties to even: a NaN quieted and its payload's top
/// kept.
pub(super) fn f16c_to_half(f: f32) -> u16 {
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
        // A normal half, or past the largest.
        let half_exp = (exp - 112) as u32;
        let mut h = (half_exp << 10) | (mant >> 13);
        let rest = mant & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && (h & 1) != 0) {
            h += 1;
        }
        if h >= 0x7c00 {
            h = 0x7c00;
        }
        return (sign | h) as u16;
    }
    if exp < 102 {
        // Under half the smallest denormal (or exactly a power of two that small).
        // 2^-25 is at exp 102: a tie with zero, which goes to zero (even).
        return sign as u16;
    }
    // A denormal half: the value in units of 2^-24, rounded.
    let m = mant | 0x80_0000; // the value is m * 2^(exp - 150)
    let shift = (126 - exp) as u32; // 13 + (113 - exp), from 14 to 24
    let mut h = m >> shift;
    let rest = m & ((1 << shift) - 1);
    let halfway = 1u32 << (shift - 1);
    if rest > halfway || (rest == halfway && (h & 1) != 0) {
        h += 1;
    }
    (sign | h) as u16
}

// ---------------------------------------------------------------------------------------
// The colour conversion.

/// `csc709Inverse`, for a block of one value: Y'CbCr to R'G'B'.
pub(super) fn csc709_inverse(data: &mut [[f32; 64]], idx: [usize; 3]) {
    let src0 = data[idx[0]][0];
    let src1 = data[idx[1]][0];
    let src2 = data[idx[2]][0];
    data[idx[0]][0] = src0 + 1.5747f32 * src2;
    data[idx[1]][0] = src0 - 0.1873f32 * src1 - 0.4682f32 * src2;
    data[idx[2]][0] = src0 + 1.8556f32 * src1;
}

/// `csc709Inverse64`, the SSE2 one. (The three blocks may be one block, which a file's
/// rules for its channels can have; each step reads it as it then is.)
pub(super) fn csc709_inverse64(data: &mut [[f32; 64]], idx: [usize; 3]) {
    let (c0, c1, c2, c3) = (1.5747f32, 1.8556f32, -0.1873f32, -0.4682f32);
    let [r, g, b] = idx;
    for i in 0..64 {
        let src0 = data[r][i];
        let src1 = data[g][i];
        let mut src2 = data[b][i];
        data[r][i] = add(data[r][i], mul(src2, c0));
        let mut gi = mul(data[g][i], c2);
        src2 = mul(src2, c3);
        gi = add(gi, src0);
        gi = add(gi, src2);
        data[g][i] = gi;
        let mut bi = mul(c1, src1);
        bi = add(bi, src0);
        data[b][i] = bi;
    }
}

/// `csc709Forward64`: R'G'B' to Y'CbCr.
pub(super) fn csc709_forward64(data: &mut [[f32; 64]], idx: [usize; 3]) {
    for i in 0..64 {
        let src0 = data[idx[0]][i];
        let src1 = data[idx[1]][i];
        let src2 = data[idx[2]][i];
        data[idx[0]][i] = 0.2126f32 * src0 + 0.7152f32 * src1 + 0.0722f32 * src2;
        data[idx[1]][i] = -0.1146f32 * src0 - 0.3854f32 * src1 + 0.5000f32 * src2;
        data[idx[2]][i] = 0.5000f32 * src0 - 0.4542f32 * src1 - 0.0458f32 * src2;
    }
}

// ---------------------------------------------------------------------------------------
// Zig-zag order.

/// Where each value of a block in zig-zag order goes in the block.
pub(super) const INV_REMAP: [usize; 64] = [
    0, 1, 5, 6, 14, 15, 27, 28, 2, 4, 7, 13, 16, 26, 29, 42, 3, 8, 12, 17, 25, 30, 41, 43, 9, 11, 18, 24, 31, 40, 44, 53, 10, 19, 23, 32, 39, 45, 52, 54, 20, 22, 33, 38, 46, 51, 55, 60, 21, 34, 37, 47, 50, 56, 59, 61, 35, 36, 48, 49,
    57, 58, 62, 63,
];

/// `fromHalfZigZag_f16c`: a block of halves in zig-zag order, as floats in order.
pub(super) fn from_half_zig_zag(src: &[u16; 64], dst: &mut [f32; 64]) {
    for (i, d) in dst.iter_mut().enumerate() {
        *d = f16c_to_float(src[INV_REMAP[i]]);
    }
}

/// `convertFloatToHalf64_f16c`.
pub(super) fn convert_float_to_half64(dst: &mut [u16], src: &[f32; 64]) {
    for (d, &s) in dst.iter_mut().zip(src) {
        *d = f16c_to_half(s);
    }
}

// ---------------------------------------------------------------------------------------
// The inverse DCT.

const A: f32 = 3.535536e-01;
const C: f32 = 4.619398e-01;
const F: f32 = 1.913422e-01;
const B: f32 = 4.903927e-01;
const D: f32 = 4.157349e-01;
const E: f32 = 2.777855e-01;
const G: f32 = 9.754573e-02;

/// The columns of M1 and of M2 (`sAvxCoef`).
const M1: [[f32; 4]; 4] = [[A, A, A, A], [C, F, -F, -C], [A, -A, -A, A], [F, -C, C, -F]];
const M2: [[f32; 4]; 4] = [[B, D, E, G], [D, -G, -B, -E], [E, -B, G, D], [G, -E, D, -B]];

/// `dctInverse8x8DcOnly`.
pub(super) fn dct_inverse8x8_dc_only(data: &mut [f32; 64]) {
    let val = mul(mul(data[0], 3.535536e-01), 3.535536e-01);
    data.fill(val);
}

/// The row pass of the AVX inverse DCT on one row.
fn idct_row(row: &[f32]) -> [f32; 8] {
    let mut out = [0f32; 8];
    for j in 0..4 {
        let even = add(add(mul(M1[0][j], row[0]), mul(M1[1][j], row[2])), add(mul(M1[2][j], row[4]), mul(M1[3][j], row[6])));
        let odd = add(add(mul(M2[0][j], row[1]), mul(M2[1][j], row[3])), add(mul(M2[2][j], row[5]), mul(M2[3][j], row[7])));
        out[j] = add(even, odd);
        out[7 - j] = sub(even, odd);
    }
    out
}

/// `dctInverse8x8_avx_<zeroed_rows>`: the last `zeroed_rows` rows of the block are taken to
/// be zero (and are left out of the sums), from 0 to 7.
pub(super) fn dct_inverse8x8(data: &mut [f32; 64], zeroed_rows: usize) {
    if zeroed_rows == 7 {
        let row = idct_row(&data[0..8]);
        for y in 0..8 {
            for x in 0..8 {
                data[y * 8 + x] = mul(row[x], A);
            }
        }
        return;
    }

    // The rows go through in pairs; a pair is left out when its first row is.
    let rows = 8 - zeroed_rows; // 2..=8
    let has = |r: usize| r < rows;
    let mut y = [[0f32; 8]; 8];
    for pair in 0..4 {
        if has(2 * pair) {
            y[2 * pair] = idct_row(&data[16 * pair..16 * pair + 8]);
            y[2 * pair + 1] = idct_row(&data[16 * pair + 8..16 * pair + 16]);
        }
    }

    let mut out = [[0f32; 8]; 8];
    for x in 0..8 {
        let v = |r: usize| y[r][x];

        // The odd outputs.
        let mut odd0 = mul(B, v(1));
        if has(3) {
            let t13 = mul(D, v(3));
            odd0 = add(t13, odd0);
        }
        if has(5) {
            let mut t14 = mul(E, v(5));
            if has(7) {
                t14 = add(mul(G, v(7)), t14);
            }
            odd0 = add(t14, odd0);
        }

        let mut odd1 = mul(D, v(1));
        if has(3) {
            let mut t14 = mul(G, v(3));
            if has(5) {
                t14 = add(mul(B, v(5)), t14);
            }
            odd1 = sub(odd1, t14);
        }
        if has(7) {
            odd1 = sub(odd1, mul(E, v(7)));
        }

        let mut odd2 = mul(E, v(1));
        if has(3) {
            odd2 = sub(odd2, mul(B, v(3)));
        }
        if has(5) {
            odd2 = add(odd2, mul(G, v(5)));
        }
        if has(7) {
            odd2 = add(odd2, mul(D, v(7)));
        }

        let mut odd3 = mul(G, v(1));
        if has(3) {
            let mut t3 = mul(E, v(3));
            if has(5) {
                odd3 = add(mul(D, v(5)), odd3);
            }
            if has(7) {
                t3 = add(mul(B, v(7)), t3);
            }
            odd3 = sub(odd3, t3);
        }

        // The even outputs.
        let a0 = mul(v(0), A);
        let (p, m) = if has(4) {
            let a4 = mul(v(4), A);
            (add(a0, a4), sub(a0, a4))
        } else {
            (a0, a0)
        };
        let (e0, e3, e1, e2) = if has(2) {
            let mut q = mul(v(2), C);
            if has(6) {
                q = add(q, mul(v(6), F));
            }
            let mut r = mul(v(2), F);
            if has(6) {
                r = sub(r, mul(v(6), C));
            }
            (add(p, q), sub(p, q), add(m, r), sub(m, r))
        } else {
            (p, p, m, m)
        };

        out[0][x] = add(e0, odd0);
        out[1][x] = add(e1, odd1);
        out[2][x] = add(e2, odd2);
        out[3][x] = add(e3, odd3);
        out[7][x] = sub(e0, odd0);
        out[6][x] = sub(e1, odd1);
        out[5][x] = sub(e2, odd2);
        out[4][x] = sub(e3, odd3);
    }
    for (r, row) in out.iter().enumerate() {
        data[8 * r..8 * r + 8].copy_from_slice(row);
    }
}

// ---------------------------------------------------------------------------------------
// The forward DCT.

/// The 1D DCT of `dctForward8x8` (SSE2) down one column of eight values. (Its constants are
/// the original's.)
#[allow(clippy::approx_constant, clippy::excessive_precision)]
fn fdct_column(s: [f32; 8]) -> [f32; 8] {
    let c4 = 0.70710678f32;
    let c4_neg = -0.70710678f32;
    let c1_half = 0.490392640f32;
    let c2_half = 0.461939770f32;
    let c3_half = 0.415734810f32;
    let c5_half = 0.277785120f32;
    let c6_half = 0.191341720f32;
    let c7_half = 0.097545161f32;
    let half = 0.5f32;

    let a0 = add(s[0], s[7]);
    let a1 = add(s[1], s[2]);
    let a3 = add(s[3], s[4]);
    let a5 = add(s[5], s[6]);
    let a7 = sub(s[0], s[7]);
    let a2 = sub(s[1], s[2]);
    let a4 = sub(s[3], s[4]);
    let a6 = sub(s[5], s[6]);

    let mut out = [0f32; 8];
    let k0 = mul(c4, add(a0, a3));
    let k1 = mul(c4, add(a1, a5));
    out[0] = mul(add(k0, k1), half);
    out[4] = mul(sub(k0, k1), half);

    let k0 = sub(a2, a6);
    let k1 = sub(a0, a3);
    out[2] = add(mul(c6_half, k0), mul(c2_half, k1));
    out[6] = sub(mul(c6_half, k1), mul(c2_half, k0));

    let k0 = mul(sub(a1, a5), c4);
    let k1 = mul(add(a2, a6), c4_neg);

    let rot_x = sub(a7, k0);
    let rot_y = add(a4, k1);
    out[3] = sub(mul(c3_half, rot_x), mul(c5_half, rot_y));
    out[5] = add(mul(c5_half, rot_x), mul(c3_half, rot_y));

    let rot_x = add(a7, k0);
    let rot_y = sub(k1, a4);
    out[1] = sub(mul(c1_half, rot_x), mul(c7_half, rot_y));
    out[7] = add(mul(c7_half, rot_x), mul(c1_half, rot_y));
    out
}

/// `dctForward8x8`, the SSE2 one: down the columns, then (after a transpose) down them
/// again, and transposed back.
pub(super) fn dct_forward8x8(data: &mut [f32; 64]) {
    for _ in 0..2 {
        let mut t = [0f32; 64];
        for x in 0..8 {
            let column: [f32; 8] = std::array::from_fn(|y| data[y * 8 + x]);
            let out = fdct_column(column);
            for (y, &value) in out.iter().enumerate() {
                // (Written transposed.)
                t[x * 8 + y] = value;
            }
        }
        *data = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_are_converted_as_openexrcore_converts_them() {
        // FNV-1a of every half as a float, and of a sweep of floats as halves, from a
        // build of OpenEXRCore (and of the F16C instructions).
        let mut hash = 14695981039346656037u64;
        let mut fnv = |bytes: &[u8]| {
            for &b in bytes {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(1099511628211);
            }
        };
        for h in 0..=u16::MAX {
            fnv(&half_to_float(h).to_bits().to_le_bytes());
        }
        assert_eq!(hash, 0xb0659868ec053145);
        let mut hash = 14695981039346656037u64;
        let mut fnv = |bytes: &[u8]| {
            for &b in bytes {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(1099511628211);
            }
        };
        for h in 0..=u16::MAX {
            fnv(&f16c_to_float(h).to_bits().to_le_bytes());
        }
        assert_eq!(hash, 0x5d79f1b086f30345);
    }

    // Every float: slow but for an optimised build.
    #[test]
    #[ignore]
    fn every_float_is_converted_as_openexrcore_and_f16c_convert_it() {
        let (mut ours, mut f16c) = (14695981039346656037u64, 14695981039346656037u64);
        let fnv = |hash: &mut u64, bytes: [u8; 2]| {
            for b in bytes {
                *hash ^= u64::from(b);
                *hash = hash.wrapping_mul(1099511628211);
            }
        };
        for bits in 0..=u32::MAX {
            let f = f32::from_bits(bits);
            fnv(&mut ours, float_to_half(f).to_le_bytes());
            fnv(&mut f16c, f16c_to_half(f).to_le_bytes());
        }
        assert_eq!((ours, f16c), (0x94a80fad4f52a325, 0xe063384da55e2325));
    }
}

