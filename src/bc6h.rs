//! BC6H, the block format for HDR colour: 4x4 pixels of RGB as half floats, 16 bytes a block.
//!
//! Unpacking is the port of the BC6H part of `bcdec.h` (Sergii Kudlai's
//! [bcdec](https://github.com/iOrange/bcdec), MIT or public domain): [`unpack_bc6h_half`] is
//! `bcdec_bc6h_half` and [`unpack_bc6h_float`] is `bcdec_bc6h_float`, for both kinds of BC6H,
//! the unsigned (`BC6H_UF16`) and the signed (`BC6H_SF16`).
//!
//! Packing is the port of the BC6H encoder of Intel's
//! [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor) (`kernel.ispc`,
//! Copyright 2017 Intel Corporation, MIT): [`compress_block_bc6h`] with the original's five
//! profiles in [`Bc6hEncSettings`]. It packs the unsigned kind only, as the original does.
//!
//! The ISPC original packs several blocks at once, one in each lane of a vector register; this
//! packs them one after another, with the same arithmetic in the same order. Built without
//! fast-math and without fused multiply-add, the original gives the same bytes on every
//! instruction set and whatever blocks are packed beside a block, and those are the bytes this
//! gives (`compare/compare.sh` checks it).
//!
//! One thing the original does is worth knowing, and is kept because blocks depend on it: it
//! keeps each pixel's squared error as an int. For bright blocks the error does not fit and
//! counts as a large negative number, so among such blocks the packer's choices are less good
//! than its search would allow.

/// A pixel of HDR colour as half floats: red, green, blue, each the 16 bits of an IEEE half.
pub type RgbHalf = [u16; 3];

/// The 128 bits of a block, read from the low end: `bcdec__bitstream_t`.
struct Bitstream {
    low: u64,
    high: u64,
}

impl Bitstream {
    fn read_bits(&mut self, num_bits: u32) -> i32 {
        let mask = (1u32 << num_bits) - 1;
        // Read the low N bits
        let bits = (self.low & u64::from(mask)) as u32;

        self.low >>= num_bits;
        // Put the low N bits of "high" into the high 64-N bits of "low".
        self.low |= (self.high & u64::from(mask)) << (64 - num_bits);
        self.high >>= num_bits;

        bits as i32
    }

    fn read_bit(&mut self) -> i32 {
        self.read_bits(1)
    }

    /// Reversed bits pulling, used in BC6H decoding.
    fn read_bits_r(&mut self, mut num_bits: u32) -> i32 {
        let mut bits = self.read_bits(num_bits);
        // Reverse the bits.
        let mut result = 0;
        while num_bits != 0 {
            num_bits -= 1;
            result <<= 1;
            result |= bits & 1;
            bits >>= 1;
        }
        result
    }
}

// http://graphics.stanford.edu/~seander/bithacks.html#VariableSignExtend
fn extend_sign(val: i32, bits: i32) -> i32 {
    (val << (32 - bits)) >> (32 - bits)
}

fn transform_inverse(mut val: i32, a0: i32, bits: i32, is_signed: bool) -> i32 {
    // If the precision of A0 is "p" bits, then the transform algorithm is:
    // B0 = (B0 + A0) & ((1 << p) - 1)
    val = val.wrapping_add(a0) & ((1 << bits) - 1);
    if is_signed {
        val = extend_sign(val, bits);
    }
    val
}

// pretty much copy-paste from documentation
fn unquantize(mut val: i32, bits: i32, is_signed: bool) -> i32 {
    let unq;
    let mut s = false;

    if !is_signed {
        if bits >= 15 {
            unq = val;
        } else if val == 0 {
            unq = 0;
        } else if val == (1 << bits) - 1 {
            unq = 0xFFFF;
        } else {
            unq = ((val << 16) + 0x8000) >> bits;
        }
    } else if bits >= 16 {
        unq = val;
    } else {
        if val < 0 {
            s = true;
            val = -val;
        }

        let magnitude = if val == 0 {
            0
        } else if val >= (1 << (bits - 1)) - 1 {
            0x7FFF
        } else {
            ((val << 15) + 0x4000) >> (bits - 1)
        };

        unq = if s { -magnitude } else { magnitude };
    }
    unq
}

fn interpolate(a: i32, b: i32, weights: &[i32], index: usize) -> i32 {
    (a * (64 - weights[index]) + b * weights[index] + 32) >> 6
}

fn finish_unquantize(mut val: i32, is_signed: bool) -> u16 {
    if !is_signed {
        ((val * 31) >> 6) as u16 // scale the magnitude by 31 / 64
    } else {
        val = if val < 0 {
            -(((-val) * 31) >> 5)
        } else {
            (val * 31) >> 5
        }; // scale the magnitude by 31 / 32
        let mut s = 0;
        if val < 0 {
            s = 0x8000;
            val = -val;
        }
        (s | val) as u16
    }
}

/// A half float as a float: `bcdec__half_to_float_quick` (after half_to_float_fast4 of
/// <https://gist.github.com/rygorous/2144712>).
pub fn half_to_float(half: u16) -> f32 {
    const MAGIC: u32 = 113 << 23;
    const SHIFTED_EXP: u32 = 0x7c00 << 13; // exponent mask after shift

    let mut o = u32::from(half & 0x7fff) << 13; // exponent/mantissa bits
    let exp = SHIFTED_EXP & o; // just the exponent
    o = o.wrapping_add((127 - 15) << 23); // exponent adjust

    // handle exponent special cases
    if exp == SHIFTED_EXP {
        // Inf/NaN?
        o = o.wrapping_add((128 - 16) << 23); // extra exp adjust
    } else if exp == 0 {
        // Zero/Denormal?
        o = o.wrapping_add(1 << 23); // extra exp adjust
        o = (f32::from_bits(o) - f32::from_bits(MAGIC)).to_bits(); // renormalize
    }

    o |= u32::from(half & 0x8000) << 16; // sign bit
    f32::from_bits(o)
}

/// A float as the nearest half float, ties to even; what is too large becomes infinity. (The
/// crate's own: bcdec only goes the other way.)
pub fn float_to_half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;

    if exponent == 255 {
        // Infinity, or not a number (kept not a number).
        return sign
            | 0x7c00
            | if mantissa != 0 {
                0x200 | (mantissa >> 13) as u16
            } else {
                0
            };
    }

    let half_exponent = exponent - 127 + 15;
    if half_exponent >= 31 {
        return sign | 0x7c00;
    }

    if half_exponent <= 0 {
        // A half denormal, or nothing.
        if half_exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x80_0000;
        let shift = (14 - half_exponent) as u32;
        let mut half = mantissa >> shift;
        let (rest, halfway) = (mantissa & ((1 << shift) - 1), 1u32 << (shift - 1));
        if rest > halfway || (rest == halfway && half & 1 == 1) {
            half += 1;
        }
        return sign | half as u16;
    }

    let mut half = ((half_exponent as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && half & 1 == 1) {
        // (A carry out of the mantissa goes into the exponent, as it should.)
        half += 1;
    }
    sign | half as u16
}

#[rustfmt::skip]
static ACTUAL_BITS_COUNT: [[i32; 14]; 4] = [
    [10, 7, 11, 11, 11, 9, 8, 8, 8, 6, 10, 11, 12, 16], //  W
    [ 5, 6,  5,  4,  4, 5, 6, 5, 5, 6, 10,  9,  8,  4], // dR
    [ 5, 6,  4,  5,  4, 5, 5, 6, 5, 6, 10,  9,  8,  4], // dG
    [ 5, 6,  4,  4,  5, 5, 5, 5, 6, 6, 10,  9,  8,  4], // dB
];

// There are 32 possible partition sets for a two-region tile.
// Each 4x4 block represents a single shape.
// Here also every fix-up index has MSB bit set.
#[rustfmt::skip]
static PARTITION_SETS: [[[u8; 4]; 4]; 32] = [
    [[128, 0,   1, 1], [0, 0, 1, 1], [  0, 0, 1, 1], [0, 0, 1, 129]], // 0
    [[128, 0,   0, 1], [0, 0, 0, 1], [  0, 0, 0, 1], [0, 0, 0, 129]], // 1
    [[128, 1,   1, 1], [0, 1, 1, 1], [  0, 1, 1, 1], [0, 1, 1, 129]], // 2
    [[128, 0,   0, 1], [0, 0, 1, 1], [  0, 0, 1, 1], [0, 1, 1, 129]], // 3
    [[128, 0,   0, 0], [0, 0, 0, 1], [  0, 0, 0, 1], [0, 0, 1, 129]], // 4
    [[128, 0,   1, 1], [0, 1, 1, 1], [  0, 1, 1, 1], [1, 1, 1, 129]], // 5
    [[128, 0,   0, 1], [0, 0, 1, 1], [  0, 1, 1, 1], [1, 1, 1, 129]], // 6
    [[128, 0,   0, 0], [0, 0, 0, 1], [  0, 0, 1, 1], [0, 1, 1, 129]], // 7
    [[128, 0,   0, 0], [0, 0, 0, 0], [  0, 0, 0, 1], [0, 0, 1, 129]], // 8
    [[128, 0,   1, 1], [0, 1, 1, 1], [  1, 1, 1, 1], [1, 1, 1, 129]], // 9
    [[128, 0,   0, 0], [0, 0, 0, 1], [  0, 1, 1, 1], [1, 1, 1, 129]], // 10
    [[128, 0,   0, 0], [0, 0, 0, 0], [  0, 0, 0, 1], [0, 1, 1, 129]], // 11
    [[128, 0,   0, 1], [0, 1, 1, 1], [  1, 1, 1, 1], [1, 1, 1, 129]], // 12
    [[128, 0,   0, 0], [0, 0, 0, 0], [  1, 1, 1, 1], [1, 1, 1, 129]], // 13
    [[128, 0,   0, 0], [1, 1, 1, 1], [  1, 1, 1, 1], [1, 1, 1, 129]], // 14
    [[128, 0,   0, 0], [0, 0, 0, 0], [  0, 0, 0, 0], [1, 1, 1, 129]], // 15
    [[128, 0,   0, 0], [1, 0, 0, 0], [  1, 1, 1, 0], [1, 1, 1, 129]], // 16
    [[128, 1, 129, 1], [0, 0, 0, 1], [  0, 0, 0, 0], [0, 0, 0,   0]], // 17
    [[128, 0,   0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 1,   0]], // 18
    [[128, 1, 129, 1], [0, 0, 1, 1], [  0, 0, 0, 1], [0, 0, 0,   0]], // 19
    [[128, 0, 129, 1], [0, 0, 0, 1], [  0, 0, 0, 0], [0, 0, 0,   0]], // 20
    [[128, 0,   0, 0], [1, 0, 0, 0], [129, 1, 0, 0], [1, 1, 1,   0]], // 21
    [[128, 0,   0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0,   0]], // 22
    [[128, 1,   1, 1], [0, 0, 1, 1], [  0, 0, 1, 1], [0, 0, 0, 129]], // 23
    [[128, 0, 129, 1], [0, 0, 0, 1], [  0, 0, 0, 1], [0, 0, 0,   0]], // 24
    [[128, 0,   0, 0], [1, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0,   0]], // 25
    [[128, 1, 129, 0], [0, 1, 1, 0], [  0, 1, 1, 0], [0, 1, 1,   0]], // 26
    [[128, 0, 129, 1], [0, 1, 1, 0], [  0, 1, 1, 0], [1, 1, 0,   0]], // 27
    [[128, 0,   0, 1], [0, 1, 1, 1], [129, 1, 1, 0], [1, 0, 0,   0]], // 28
    [[128, 0,   0, 0], [1, 1, 1, 1], [129, 1, 1, 1], [0, 0, 0,   0]], // 29
    [[128, 1, 129, 1], [0, 0, 0, 1], [  1, 0, 0, 0], [1, 1, 1,   0]], // 30
    [[128, 0, 129, 1], [1, 0, 0, 1], [  1, 0, 0, 1], [1, 1, 0,   0]], // 31
];

static A_WEIGHT3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
static A_WEIGHT4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Unpacks a BC6H block to 16 pixels of half floats, in rows from the top. `is_signed` says
/// which of the two kinds of BC6H the block is. A block of a reserved mode unpacks to black.
pub fn unpack_bc6h_half(block: &[u8; 16], is_signed: bool) -> [RgbHalf; 16] {
    let mut decompressed = [[0u16; 3]; 16];

    let mut bstream = Bitstream {
        low: u64::from_le_bytes(block[..8].try_into().expect("eight bytes")),
        high: u64::from_le_bytes(block[8..].try_into().expect("eight bytes")),
    };

    let (mut r, mut g, mut b) = ([0i32; 4], [0i32; 4], [0i32; 4]); // wxyz

    let mut mode_bits = bstream.read_bits(2);
    if mode_bits > 1 {
        mode_bits |= bstream.read_bits(3) << 2;
    }

    // modes >= 11 (10 in my code) are using 0 one, others will read it from the bitstream
    let mut partition = 0;
    let mode: usize;

    match mode_bits {
        // mode 1
        0b00 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 75 bits (10.555, 10.555, 10.555)
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[2] |= bstream.read_bit() << 4; // by[4]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(5); // rx[4:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(5); // gx[4:0]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(5); // bx[4:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(5); // ry[4:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(5); // rz[4:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 0;
        }
        // mode 2
        0b01 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 75 bits (7666, 7666, 7666)
            g[2] |= bstream.read_bit() << 5; // gy[5]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[3] |= bstream.read_bit() << 5; // gz[5]
            r[0] |= bstream.read_bits(7); // rw[6:0]
            b[3] |= bstream.read_bit(); // bz[0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(7); // gw[6:0]
            b[2] |= bstream.read_bit() << 5; // by[5]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(7); // bw[6:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            b[3] |= bstream.read_bit() << 5; // bz[5]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(6); // rx[5:0]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(6); // gx[5:0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(6); // bx[5:0]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(6); // ry[5:0]
            r[3] |= bstream.read_bits(6); // rz[5:0]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 1;
        }
        // mode 3
        0b00010 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (11.555, 11.444, 11.444)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(5); // rx[4:0]
            r[0] |= bstream.read_bit() << 10; // rw[10]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(4); // gx[3:0]
            g[0] |= bstream.read_bit() << 10; // gw[10]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(4); // bx[3:0]
            b[0] |= bstream.read_bit() << 10; // bw[10]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(5); // ry[4:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(5); // rz[4:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 2;
        }
        // mode 4
        0b00110 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (11.444, 11.555, 11.444)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(4); // rx[3:0]
            r[0] |= bstream.read_bit() << 10; // rw[10]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(5); // gx[4:0]
            g[0] |= bstream.read_bit() << 10; // gw[10]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(4); // bx[3:0]
            b[0] |= bstream.read_bit() << 10; // bw[10]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(4); // ry[3:0]
            b[3] |= bstream.read_bit(); // bz[0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(4); // rz[3:0]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 3;
        }
        // mode 5
        0b01010 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (11.444, 11.444, 11.555)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(4); // rx[3:0]
            r[0] |= bstream.read_bit() << 10; // rw[10]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(4); // gx[3:0]
            g[0] |= bstream.read_bit() << 10; // gw[10]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(5); // bx[4:0]
            b[0] |= bstream.read_bit() << 10; // bw[10]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(4); // ry[3:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(4); // rz[3:0]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 4;
        }
        // mode 6
        0b01110 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (9555, 9555, 9555)
            r[0] |= bstream.read_bits(9); // rw[8:0]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(9); // gw[8:0]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(9); // bw[8:0]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(5); // rx[4:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(5); // gx[4:0]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gx[3:0]
            b[1] |= bstream.read_bits(5); // bx[4:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(5); // ry[4:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(5); // rz[4:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 5;
        }
        // mode 7
        0b10010 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (8666, 8555, 8555)
            r[0] |= bstream.read_bits(8); // rw[7:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(8); // gw[7:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(8); // bw[7:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(6); // rx[5:0]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(5); // gx[4:0]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(5); // bx[4:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(6); // ry[5:0]
            r[3] |= bstream.read_bits(6); // rz[5:0]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 6;
        }
        // mode 8
        0b10110 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (8555, 8666, 8555)
            r[0] |= bstream.read_bits(8); // rw[7:0]
            b[3] |= bstream.read_bit(); // bz[0]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(8); // gw[7:0]
            g[2] |= bstream.read_bit() << 5; // gy[5]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(8); // bw[7:0]
            g[3] |= bstream.read_bit() << 5; // gz[5]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(5); // rx[4:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(6); // gx[5:0]
            g[3] |= bstream.read_bits(4); // zx[3:0]
            b[1] |= bstream.read_bits(5); // bx[4:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(5); // ry[4:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(5); // rz[4:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 7;
        }
        // mode 9
        0b11010 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (8555, 8555, 8666)
            r[0] |= bstream.read_bits(8); // rw[7:0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(8); // gw[7:0]
            b[2] |= bstream.read_bit() << 5; // by[5]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(8); // bw[7:0]
            b[3] |= bstream.read_bit() << 5; // bz[5]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(5); // bw[4:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(5); // gx[4:0]
            b[3] |= bstream.read_bit(); // bz[0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(6); // bx[5:0]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(5); // ry[4:0]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            r[3] |= bstream.read_bits(5); // rz[4:0]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 8;
        }
        // mode 10
        0b11110 => {
            // Partitition indices: 46 bits
            // Partition: 5 bits
            // Color Endpoints: 72 bits (6666, 6666, 6666)
            r[0] |= bstream.read_bits(6); // rw[5:0]
            g[3] |= bstream.read_bit() << 4; // gz[4]
            b[3] |= bstream.read_bit(); // bz[0]
            b[3] |= bstream.read_bit() << 1; // bz[1]
            b[2] |= bstream.read_bit() << 4; // by[4]
            g[0] |= bstream.read_bits(6); // gw[5:0]
            g[2] |= bstream.read_bit() << 5; // gy[5]
            b[2] |= bstream.read_bit() << 5; // by[5]
            b[3] |= bstream.read_bit() << 2; // bz[2]
            g[2] |= bstream.read_bit() << 4; // gy[4]
            b[0] |= bstream.read_bits(6); // bw[5:0]
            g[3] |= bstream.read_bit() << 5; // gz[5]
            b[3] |= bstream.read_bit() << 3; // bz[3]
            b[3] |= bstream.read_bit() << 5; // bz[5]
            b[3] |= bstream.read_bit() << 4; // bz[4]
            r[1] |= bstream.read_bits(6); // rx[5:0]
            g[2] |= bstream.read_bits(4); // gy[3:0]
            g[1] |= bstream.read_bits(6); // gx[5:0]
            g[3] |= bstream.read_bits(4); // gz[3:0]
            b[1] |= bstream.read_bits(6); // bx[5:0]
            b[2] |= bstream.read_bits(4); // by[3:0]
            r[2] |= bstream.read_bits(6); // ry[5:0]
            r[3] |= bstream.read_bits(6); // rz[5:0]
            partition = bstream.read_bits(5); // d[4:0]
            mode = 9;
        }
        // mode 11
        0b00011 => {
            // Partitition indices: 63 bits
            // Partition: 0 bits
            // Color Endpoints: 60 bits (10.10, 10.10, 10.10)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(10); // rx[9:0]
            g[1] |= bstream.read_bits(10); // gx[9:0]
            b[1] |= bstream.read_bits(10); // bx[9:0]
            mode = 10;
        }
        // mode 12
        0b00111 => {
            // Partitition indices: 63 bits
            // Partition: 0 bits
            // Color Endpoints: 60 bits (11.9, 11.9, 11.9)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(9); // rx[8:0]
            r[0] |= bstream.read_bit() << 10; // rw[10]
            g[1] |= bstream.read_bits(9); // gx[8:0]
            g[0] |= bstream.read_bit() << 10; // gw[10]
            b[1] |= bstream.read_bits(9); // bx[8:0]
            b[0] |= bstream.read_bit() << 10; // bw[10]
            mode = 11;
        }
        // mode 13
        0b01011 => {
            // Partitition indices: 63 bits
            // Partition: 0 bits
            // Color Endpoints: 60 bits (12.8, 12.8, 12.8)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(8); // rx[7:0]
            r[0] |= bstream.read_bits_r(2) << 10; // rx[10:11]
            g[1] |= bstream.read_bits(8); // gx[7:0]
            g[0] |= bstream.read_bits_r(2) << 10; // gx[10:11]
            b[1] |= bstream.read_bits(8); // bx[7:0]
            b[0] |= bstream.read_bits_r(2) << 10; // bx[10:11]
            mode = 12;
        }
        // mode 14
        0b01111 => {
            // Partitition indices: 63 bits
            // Partition: 0 bits
            // Color Endpoints: 60 bits (16.4, 16.4, 16.4)
            r[0] |= bstream.read_bits(10); // rw[9:0]
            g[0] |= bstream.read_bits(10); // gw[9:0]
            b[0] |= bstream.read_bits(10); // bw[9:0]
            r[1] |= bstream.read_bits(4); // rx[3:0]
            r[0] |= bstream.read_bits_r(6) << 10; // rw[10:15]
            g[1] |= bstream.read_bits(4); // gx[3:0]
            g[0] |= bstream.read_bits_r(6) << 10; // gw[10:15]
            b[1] |= bstream.read_bits(4); // bx[3:0]
            b[0] |= bstream.read_bits_r(6) << 10; // bw[10:15]
            mode = 13;
        }
        _ => {
            // Modes 10011, 10111, 11011, and 11111 (not shown) are reserved.
            // Do not use these in your encoder. If the hardware is passed blocks
            // with one of these modes specified, the resulting decompressed block
            // must contain all zeroes in all channels except for the alpha channel.
            return decompressed;
        }
    }

    let num_partitions = if mode >= 10 { 0 } else { 1 };

    let actual_bits0_mode = ACTUAL_BITS_COUNT[0][mode];
    if is_signed {
        r[0] = extend_sign(r[0], actual_bits0_mode);
        g[0] = extend_sign(g[0], actual_bits0_mode);
        b[0] = extend_sign(b[0], actual_bits0_mode);
    }

    // Mode 11 (like Mode 10) does not use delta compression,
    // and instead stores both color endpoints explicitly.
    if (mode != 9 && mode != 10) || is_signed {
        for i in 1..(num_partitions + 1) * 2 {
            r[i] = extend_sign(r[i], ACTUAL_BITS_COUNT[1][mode]);
            g[i] = extend_sign(g[i], ACTUAL_BITS_COUNT[2][mode]);
            b[i] = extend_sign(b[i], ACTUAL_BITS_COUNT[3][mode]);
        }
    }

    if mode != 9 && mode != 10 {
        for i in 1..(num_partitions + 1) * 2 {
            r[i] = transform_inverse(r[i], r[0], actual_bits0_mode, is_signed);
            g[i] = transform_inverse(g[i], g[0], actual_bits0_mode, is_signed);
            b[i] = transform_inverse(b[i], b[0], actual_bits0_mode, is_signed);
        }
    }

    for i in 0..(num_partitions + 1) * 2 {
        r[i] = unquantize(r[i], actual_bits0_mode, is_signed);
        g[i] = unquantize(g[i], actual_bits0_mode, is_signed);
        b[i] = unquantize(b[i], actual_bits0_mode, is_signed);
    }

    let weights: &[i32] = if mode >= 10 { &A_WEIGHT4 } else { &A_WEIGHT3 };
    for i in 0..4 {
        for j in 0..4 {
            let mut partition_set = if mode >= 10 {
                if (i | j) != 0 { 0 } else { 128 }
            } else {
                PARTITION_SETS[partition as usize][i][j]
            };

            let mut index_bits = if mode >= 10 { 4 } else { 3 };
            // fix-up index is specified with one less bit
            // The fix-up index for subset 0 is always index 0
            if partition_set & 0x80 != 0 {
                index_bits -= 1;
            }
            partition_set &= 0x01;

            let index = bstream.read_bits(index_bits) as usize;

            let ep_i = usize::from(partition_set) * 2;
            decompressed[i * 4 + j] = [
                finish_unquantize(interpolate(r[ep_i], r[ep_i + 1], weights, index), is_signed),
                finish_unquantize(interpolate(g[ep_i], g[ep_i + 1], weights, index), is_signed),
                finish_unquantize(interpolate(b[ep_i], b[ep_i + 1], weights, index), is_signed),
            ];
        }
    }

    decompressed
}

/// Unpacks a BC6H block to 16 pixels of floats, in rows from the top: [`unpack_bc6h_half`] with
/// each half widened ([`half_to_float`]).
pub fn unpack_bc6h_float(block: &[u8; 16], is_signed: bool) -> [[f32; 3]; 16] {
    unpack_bc6h_half(block, is_signed).map(|pixel| pixel.map(half_to_float))
}

// ---------------------------------------------------------------------------------------------
// Packing: the port of the BC6H encoder of Intel's ISPC Texture Compressor.

/// Float to int as the x86 instruction ISPC compiles a cast to: toward zero, and the lowest int
/// for what does not fit or is not a number.
fn f2i(x: f32) -> i32 {
    if x >= -2147483648.0 && x < 2147483648.0 {
        x as i32
    } else {
        i32::MIN
    }
}

/// ISPC's `min` of floats: the second unless the first is less.
fn min_f(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// ISPC's `max` of floats: the second unless the first is greater.
fn max_f(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

fn sq(v: f32) -> f32 {
    v * v
}

/// The settings of the packer: `bc6h_enc_settings`. The five profiles are the original's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bc6hEncSettings {
    /// Try every mode in full.
    pub slow_mode: bool,
    /// Leave out the two-subset modes that seldom win.
    pub fast_mode: bool,
    /// How many times a one-subset block's endpoints are refined.
    pub refine_iterations_1p: u32,
    /// How many times a two-subset block's endpoints are refined.
    pub refine_iterations_2p: u32,
    /// How many of the 32 partitions, the most promising first, are tried in full for the
    /// two-subset modes. 0 leaves those modes out.
    pub fast_skip_threshold: u32,
}

impl Bc6hEncSettings {
    /// `GetProfile_bc6h_veryfast`: one subset only.
    pub fn veryfast() -> Self {
        Bc6hEncSettings {
            slow_mode: false,
            fast_mode: true,
            fast_skip_threshold: 0,
            refine_iterations_1p: 0,
            refine_iterations_2p: 0,
        }
    }

    /// `GetProfile_bc6h_fast`.
    pub fn fast() -> Self {
        Bc6hEncSettings {
            slow_mode: false,
            fast_mode: true,
            fast_skip_threshold: 2,
            refine_iterations_1p: 0,
            refine_iterations_2p: 1,
        }
    }

    /// `GetProfile_bc6h_basic`.
    pub fn basic() -> Self {
        Bc6hEncSettings {
            slow_mode: false,
            fast_mode: false,
            fast_skip_threshold: 4,
            refine_iterations_1p: 2,
            refine_iterations_2p: 2,
        }
    }

    /// `GetProfile_bc6h_slow`.
    pub fn slow() -> Self {
        Bc6hEncSettings {
            slow_mode: true,
            fast_mode: false,
            fast_skip_threshold: 10,
            refine_iterations_1p: 2,
            refine_iterations_2p: 2,
        }
    }

    /// `GetProfile_bc6h_veryslow`.
    pub fn veryslow() -> Self {
        Bc6hEncSettings {
            slow_mode: true,
            fast_mode: false,
            fast_skip_threshold: 32,
            refine_iterations_1p: 2,
            refine_iterations_2p: 2,
        }
    }
}

/// `bc6h_enc_state`: a block on its way to being packed.
struct Bc6hEncState {
    /// The pixels, a channel after another, sixteen values each.
    block: [f32; 64],

    best_err: f32,
    best_data: [u32; 5], // 4, +1 margin for skips

    rgb_bounds: [f32; 6],
    max_span: f32,
    max_span_idx: i32,

    mode: i32,
    epb: i32,
    qbounds: [i32; 8],

    settings: Bc6hEncSettings,
}

// BC6H format data

fn get_mode_prefix(mode: usize) -> u32 {
    const MODE_PREFIX_TABLE: [u32; 14] = [0, 1, 2, 6, 10, 14, 18, 22, 26, 30, 3, 7, 11, 15];
    MODE_PREFIX_TABLE[mode]
}

/// The widest spread of values a mode can hold. (The original works these out as floats,
/// `0.9 * 0xFFFF / 64` and so on, and keeps their whole part.)
fn get_span(mode: usize) -> f32 {
    const SPAN_TABLE: [i32; 14] = [
        921,   //  (0) 4 / 10
        14745, //  (1) 5 / 7
        204,   //  (2) 3 / 11
        -1, -1,   //
        1843, //  (5) 4 / 9
        3686, //  (6) 4 / 8
        -1, -1,    //
        65535, //  (9) absolute
        65535, // (10) absolute
        7782,  // (11) 8 / 11
        1945,  // (12) 7 / 12
        6,     // (13) 3 / 16
    ];
    SPAN_TABLE[mode] as f32
}

fn get_mode_bits(mode: usize) -> i32 {
    const MODE_BITS_TABLE: [i32; 14] = [10, 7, 11, -1, -1, 9, 8, -1, -1, 6, 10, 11, 12, 16];
    MODE_BITS_TABLE[mode]
}

// The shared format data of the original's BC7 and BC6H: the first 32 partitions, of two
// subsets.

/// Two bits a pixel: which subset it is in.
#[rustfmt::skip]
static PATTERN_TABLE: [u32; 32] = [
    0x50505050, 0x40404040, 0x54545454, 0x54505040, 0x50404000, 0x55545450, 0x55545040, 0x54504000,
    0x50400000, 0x55555450, 0x55544000, 0x54400000, 0x55555440, 0x55550000, 0x55555500, 0x55000000,
    0x55150100, 0x00004054, 0x15010000, 0x00405054, 0x00004050, 0x15050100, 0x05010000, 0x40505054,
    0x00404050, 0x05010100, 0x14141414, 0x05141450, 0x01155440, 0x00555500, 0x15014054, 0x05414150,
];

/// The pixels of the first subset in the low 16 bits and of the second in the high.
#[rustfmt::skip]
static PATTERN_MASK_TABLE: [u32; 32] = [
    0xcccc3333, 0x88887777, 0xeeee1111, 0xecc81337, 0xc880377f, 0xfeec0113, 0xfec80137, 0xec80137f,
    0xc80037ff, 0xffec0013, 0xfe80017f, 0xe80017ff, 0xffe80017, 0xff0000ff, 0xfff0000f, 0xf0000fff,
    0xf71008ef, 0x008eff71, 0x71008eff, 0x08cef731, 0x008cff73, 0x73108cef, 0x3100ceff, 0x8cce7331,
    0x088cf773, 0x3110ceef, 0x66669999, 0x366cc993, 0x17e8e817, 0x0ff0f00f, 0x718e8e71, 0x399cc663,
];

/// The pixel whose index is stored one bit short in the second subset, in the high four bits.
#[rustfmt::skip]
static SKIP_TABLE: [i32; 32] = [
    0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0, 0xf0,
    0xf0, 0x20, 0x80, 0x20, 0x20, 0x80, 0x80, 0xf0, 0x20, 0x80, 0x20, 0x20, 0x80, 0x80, 0x20, 0x20,
];

fn get_pattern(part_id: i32) -> u32 {
    PATTERN_TABLE[part_id as usize]
}

fn get_pattern_mask(part_id: i32, j: usize) -> i32 {
    let mask_packed = PATTERN_MASK_TABLE[part_id as usize];
    let mask0 = (mask_packed & 0xFFFF) as i32;
    let mask1 = (mask_packed >> 16) as i32;
    if j == 2 {
        !mask0 & !mask1
    } else if j == 0 {
        mask0
    } else {
        mask1
    }
}

fn get_skips(part_id: i32) -> [i32; 3] {
    let skip_packed = SKIP_TABLE[part_id as usize];
    [0, skip_packed >> 4, skip_packed & 15]
}

static UNQUANT_TABLE_3BITS: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
static UNQUANT_TABLE_4BITS: [i32; 16] =
    [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

// PCA helpers

fn ssymv3(a: &mut [f32; 4], covar: &[f32; 10], b: &[f32; 4]) {
    a[0] = covar[0] * b[0] + covar[1] * b[1] + covar[2] * b[2];
    a[1] = covar[1] * b[0] + covar[4] * b[1] + covar[5] * b[2];
    a[2] = covar[2] * b[0] + covar[5] * b[1] + covar[7] * b[2];
}

/// The main axis of a spread of colours, by power iteration: `compute_axis` for three channels.
fn compute_axis(axis: &mut [f32; 4], covar: &[f32; 10], power_iterations: u32) {
    let mut vec = [1.0f32; 4];

    for i in 0..power_iterations {
        ssymv3(axis, covar, &vec);
        vec[..3].copy_from_slice(&axis[..3]);

        if i % 2 == 1 {
            // renormalize every other iteration
            let mut norm_sq = 0.0f32;
            for p in 0..3 {
                norm_sq += axis[p] * axis[p];
            }

            let rnorm = 1.0 / norm_sq.sqrt();
            for p in 0..3 {
                vec[p] *= rnorm;
            }
        }
    }

    axis[..3].copy_from_slice(&vec[..3]);
}

/// Sums over the pixels a mask names: of squares and products in 0 to 9, of each channel in 10
/// to 13, and the count in 14. Three channels.
fn compute_stats_masked(stats: &mut [f32; 15], block: &[f32; 64], mask: i32) {
    *stats = [0.0; 15];

    let mut mask_shifted = mask << 1;
    for k in 0..16 {
        mask_shifted >>= 1;
        // (The original multiplies a pixel outside the mask by 0 and adds it all the same. The
        // values are finite and the sums never below zero, so leaving it out gives the same
        // sums.)
        if mask_shifted & 1 == 0 {
            continue;
        }

        let rgba = [block[k], block[k + 16], block[k + 32]];
        stats[14] += 1.0;

        stats[10] += rgba[0];
        stats[11] += rgba[1];
        stats[12] += rgba[2];

        stats[0] += rgba[0] * rgba[0];
        stats[1] += rgba[0] * rgba[1];
        stats[2] += rgba[0] * rgba[2];

        stats[4] += rgba[1] * rgba[1];
        stats[5] += rgba[1] * rgba[2];

        stats[7] += rgba[2] * rgba[2];
    }
}

fn covar_from_stats(covar: &mut [f32; 10], stats: &[f32; 15]) {
    covar[0] = stats[0] - stats[10] * stats[10] / stats[14];
    covar[1] = stats[1] - stats[10] * stats[11] / stats[14];
    covar[2] = stats[2] - stats[10] * stats[12] / stats[14];

    covar[4] = stats[4] - stats[11] * stats[11] / stats[14];
    covar[5] = stats[5] - stats[11] * stats[12] / stats[14];

    covar[7] = stats[7] - stats[12] * stats[12] / stats[14];
}

fn block_pca_axis(axis: &mut [f32; 4], dc: &mut [f32; 4], block: &[f32; 64], mask: i32) {
    const POWER_ITERATIONS: u32 = 8; // 4 not enough for HQ

    let mut covar = [0.0f32; 10];
    let mut stats = [0.0f32; 15];
    compute_stats_masked(&mut stats, block, mask);
    covar_from_stats(&mut covar, &stats);
    for p in 0..3 {
        dc[p] = stats[10 + p] / stats[14];
    }

    let inv_var = 1.0f32 / (256.0 * 256.0);
    for value in &mut covar {
        *value *= inv_var;
    }

    let eps = sq(0.001);
    covar[0] += eps;
    covar[4] += eps;
    covar[7] += eps;
    covar[9] += eps;

    compute_axis(axis, &covar, POWER_ITERATIONS);
}

/// The two ends of the line through a subset's colours along their main axis.
fn block_segment_core(ep: &mut [f32], block: &[f32; 64], mask: i32) {
    let mut axis = [0.0f32; 4];
    let mut dc = [0.0f32; 4];
    block_pca_axis(&mut axis, &mut dc, block, mask);

    let mut ext = [f32::INFINITY, f32::NEG_INFINITY];

    // find min/max
    let mut mask_shifted = mask << 1;
    for k in 0..16 {
        mask_shifted >>= 1;
        if mask_shifted & 1 == 0 {
            continue;
        }

        let mut dot = 0.0f32;
        for p in 0..3 {
            dot += axis[p] * (block[16 * p + k] - dc[p]);
        }

        ext[0] = min_f(ext[0], dot);
        ext[1] = max_f(ext[1], dot);
    }

    // create some distance if the endpoints collapse
    if ext[1] - ext[0] < 1.0 {
        ext[0] -= 0.5;
        ext[1] += 0.5;
    }

    for i in 0..2 {
        for p in 0..3 {
            ep[4 * i + p] = ext[i] * axis[p] + dc[p];
        }
    }
}

fn get_pca_bound(covar: &mut [f32; 10]) -> f32 {
    const POWER_ITERATIONS: u32 = 4; // quite approximative, but enough for bounding

    let inv_var = 1.0f32 / (256.0 * 256.0);
    for value in covar.iter_mut() {
        *value *= inv_var;
    }

    let eps = sq(0.001);
    covar[0] += eps;
    covar[4] += eps;
    covar[7] += eps;

    let mut axis = [0.0f32; 4];
    compute_axis(&mut axis, covar, POWER_ITERATIONS);

    let mut vec = [0.0f32; 4];
    ssymv3(&mut vec, covar, &axis);

    let mut sq_sum = 0.0f32;
    for p in 0..3 {
        sq_sum += sq(vec[p]);
    }
    let lambda = sq_sum.sqrt();

    let mut bound = covar[0] + covar[4] + covar[7];
    bound -= lambda;
    bound = max_f(bound, 0.0);

    bound
}

fn block_pca_bound_split(block: &[f32; 64], mask: i32, full_stats: &[f32; 15]) -> f32 {
    let mut stats = [0.0f32; 15];
    compute_stats_masked(&mut stats, block, mask);

    let mut covar1 = [0.0f32; 10];
    covar_from_stats(&mut covar1, &stats);

    for i in 0..15 {
        stats[i] = full_stats[i] - stats[i];
    }

    let mut covar2 = [0.0f32; 10];
    covar_from_stats(&mut covar2, &stats);

    let mut bound = 0.0f32;
    bound += get_pca_bound(&mut covar1);
    bound += get_pca_bound(&mut covar2);

    bound.sqrt() * 256.0
}

// endpoint quantization

fn unpack_to_uf16(v: u32, bits: i32) -> i32 {
    if bits >= 15 {
        return v as i32;
    }
    if v == 0 {
        return 0;
    }
    if v == (1u32 << bits) - 1 {
        return 0xFFFF;
    }

    ((v * 2 + 1) << (15 - bits)) as i32
}

fn ep_quant_bc6h(qep: &mut [i32], ep: &[f32], bits: i32, pairs: usize) {
    let levels = 1i32 << bits;

    for i in 0..8 * pairs {
        let v = f2i(ep[i] / (256.0 * 256.0 - 1.0) * (levels - 1) as f32 + 0.5);
        qep[i] = v.max(0).min(levels - 1);
    }
}

fn ep_dequant_bc6h(ep: &mut [f32], qep: &[i32], bits: i32, pairs: usize) {
    for i in 0..8 * pairs {
        ep[i] = unpack_to_uf16(qep[i] as u32, bits) as f32;
    }
}

fn ep_quant_dequant_bc6h(state: &Bc6hEncState, qep: &mut [i32], ep: &mut [f32], pairs: usize) {
    let bits = state.epb;
    ep_quant_bc6h(qep, ep, bits, pairs);

    for i in 0..2 * pairs {
        for p in 0..3 {
            qep[i * 4 + p] = qep[i * 4 + p]
                .max(state.qbounds[p])
                .min(state.qbounds[4 + p]);
        }
    }

    ep_dequant_bc6h(ep, qep, bits, pairs);
}

/// Picks each pixel's index between its subset's two ends and gives the block's error. The
/// indices come out four bits a pixel in two words. (The original keeps a pixel's error as an
/// int, so an error too large for one counts as the lowest int: kept, as blocks depend on it.)
fn block_quant(
    qblock: &mut [u32; 2],
    block: &[f32; 64],
    bits: u32,
    ep: &[f32],
    pattern: u32,
) -> f32 {
    let mut total_err = 0.0f32;
    let unquant_table: &[i32] = if bits == 3 {
        &UNQUANT_TABLE_3BITS
    } else {
        &UNQUANT_TABLE_4BITS
    };
    let levels = 1i32 << bits;

    // What the original works out again for every pixel depends only on the pixel's subset and
    // index, so it is worked out once here, by the same arithmetic: for each subset, the step
    // from one end to the other and its squared length, and what each index unpacks to.
    let subsets = ep.len() / 8;
    let mut delta = [[0.0f32; 3]; 2];
    let mut div = [0.0f32; 2];
    let mut decoded = [[[0.0f32; 3]; 16]; 2];
    for j in 0..subsets.min(2) {
        for p in 0..3 {
            let ep_a = ep[8 * j + p];
            let ep_b = ep[8 * j + 4 + p];
            delta[j][p] = ep_b - ep_a;
            div[j] += sq(ep_b - ep_a);
            for (q, &w) in unquant_table.iter().enumerate() {
                decoded[j][q][p] =
                    f2i(((64 - w) as f32 * ep_a + w as f32 * ep_b + 32.0) / 64.0) as f32;
            }
        }
    }

    // 64-bit qblock: 5% overhead in this function
    *qblock = [0; 2];

    let mut pattern_shifted = pattern as i32;
    for k in 0..16 {
        let j = (pattern_shifted & 3) as usize;
        pattern_shifted >>= 2;

        let pixel = [block[k], block[k + 16], block[k + 32]];

        let mut proj = 0.0f32;
        for p in 0..3 {
            proj += (pixel[p] - ep[8 * j + p]) * delta[j][p];
        }

        proj /= div[j];

        let mut q1 = f2i(proj * levels as f32 + 0.5);
        q1 = q1.max(1).min(levels - 1);

        let mut err0 = 0.0f32;
        let mut err1 = 0.0f32;
        let (dec_v0, dec_v1) = (&decoded[j][(q1 - 1) as usize], &decoded[j][q1 as usize]);

        for p in 0..3 {
            err0 += sq(dec_v0[p] - pixel[p]);
            err1 += sq(dec_v1[p] - pixel[p]);
        }

        let mut best_err = f2i(err1);
        let mut best_q = q1;
        if err0 < err1 {
            best_err = f2i(err0);
            best_q = q1 - 1;
        }

        qblock[k / 8] += (best_q as u32) << (4 * (k % 8));
        total_err += best_err as f32;
    }

    total_err
}

// LS endpoint refinement

/// The two ends that fit a subset's pixels best in the least-squares sense, given their indices.
fn opt_endpoints(ep: &mut [f32], block: &[f32; 64], bits: u32, qblock: &[u32; 2], mask: i32) {
    let levels = 1i32 << bits;

    let mut atb1 = [0.0f32; 4];
    let mut sum_q = 0.0f32;
    let mut sum_qq = 0.0f32;
    let mut sum = [0.0f32; 5];

    let mut mask_shifted = mask << 1;
    for k1 in 0..2 {
        let mut qbits_shifted = qblock[k1];
        for k2 in 0..8 {
            let k = k1 * 8 + k2;
            let q = (qbits_shifted & 15) as f32;
            qbits_shifted >>= 4;

            mask_shifted >>= 1;
            if mask_shifted & 1 == 0 {
                continue;
            }

            let x = (levels - 1) - q as i32;

            sum_q += q;
            sum_qq += q * q;

            sum[4] += 1.0;
            for p in 0..3 {
                sum[p] += block[k + p * 16];
            }
            for p in 0..3 {
                atb1[p] += x as f32 * block[k + p * 16];
            }
        }
    }

    let mut atb2 = [0.0f32; 4];
    for p in 0..3 {
        atb2[p] = (levels - 1) as f32 * sum[p] - atb1[p];
    }

    let cxx = sum[4] * sq((levels - 1) as f32) - (2 * (levels - 1)) as f32 * sum_q + sum_qq;
    let cyy = sum_qq;
    let cxy = (levels - 1) as f32 * sum_q - sum_qq;
    let scale = (levels - 1) as f32 / (cxx * cyy - cxy * cxy);

    for p in 0..3 {
        ep[p] = (atb1[p] * cyy - atb2[p] * cxy) * scale;
        ep[4 + p] = (atb2[p] * cxx - atb1[p] * cxy) * scale;
    }

    if (cxx * cyy - cxy * cxy).abs() < 0.001 {
        // flatten
        for p in 0..3 {
            ep[p] = sum[p] / sum[4];
            ep[4 + p] = ep[p];
        }
    }
}

// parameter estimation

fn partial_sort_list(list: &mut [i32], length: usize, partial_count: usize) {
    for k in 0..partial_count {
        let mut best_idx = k;
        let mut best_value = list[k];
        for i in k + 1..length {
            if best_value > list[i] {
                best_value = list[i];
                best_idx = i;
            }
        }

        // swap
        list[best_idx] = list[k];
        list[k] = best_value;
    }
}

fn bc6h_enc_2p_part_fast(
    state: &Bc6hEncState,
    qep: &mut [i32; 24],
    qblock: &mut [u32; 2],
    part_id: i32,
) -> f32 {
    let pattern = get_pattern(part_id);

    let mut ep = [0.0f32; 16];
    for j in 0..2 {
        let mask = get_pattern_mask(part_id, j);
        block_segment_core(&mut ep[j * 8..], &state.block, mask);
    }

    ep_quant_dequant_bc6h(state, qep, &mut ep, 2);

    block_quant(qblock, &state.block, 3, &ep, pattern)
}

fn bc6h_enc_2p_list(state: &mut Bc6hEncState, part_list: &[i32; 32], part_count: usize) {
    if part_count == 0 {
        return;
    }
    const BITS: u32 = 3;
    const PAIRS: usize = 2;

    let mut best_qep = [0i32; 24];
    let mut best_qblock = [0u32; 2];
    let mut best_part_id = -1i32;
    let mut best_err = f32::INFINITY;

    for &listed in &part_list[..part_count] {
        let part_id = listed & 31;

        let mut qep = [0i32; 24];
        let mut qblock = [0u32; 2];
        let err = bc6h_enc_2p_part_fast(state, &mut qep, &mut qblock, part_id);

        if err < best_err {
            best_qep[..8 * PAIRS].copy_from_slice(&qep[..8 * PAIRS]);
            best_qblock = qblock;
            best_part_id = part_id;
            best_err = err;
        }
    }

    // (The original would go on with a partition of -1 had no error been below its start; none
    // is ever not, short of an error that is not a number.)
    if best_part_id < 0 {
        return;
    }

    // refine
    for _ in 0..state.settings.refine_iterations_2p {
        let mut ep = [0.0f32; 24];
        for j in 0..PAIRS {
            let mask = get_pattern_mask(best_part_id, j);
            opt_endpoints(&mut ep[j * 8..], &state.block, BITS, &best_qblock, mask);
        }

        let mut qep = [0i32; 24];
        let mut qblock = [0u32; 2];
        ep_quant_dequant_bc6h(state, &mut qep, &mut ep, 2);

        let pattern = get_pattern(best_part_id);
        let err = block_quant(&mut qblock, &state.block, BITS, &ep, pattern);

        if err < best_err {
            best_qep[..8 * PAIRS].copy_from_slice(&qep[..8 * PAIRS]);
            best_qblock = qblock;
            best_err = err;
        }
    }

    if best_err < state.best_err {
        state.best_err = best_err;
        bc6h_code_2p(
            &mut state.best_data,
            &mut best_qep,
            &mut best_qblock,
            best_part_id,
            state.mode,
        );
    }
}

fn bc6h_enc_2p(state: &mut Bc6hEncState) {
    let mut full_stats = [0.0f32; 15];
    compute_stats_masked(&mut full_stats, &state.block, -1);

    let mut part_list = [0i32; 32];
    for part in 0..32 {
        let mask = get_pattern_mask(part, 0);
        let bound12 = block_pca_bound_split(&state.block, mask, &full_stats);
        let bound = f2i(bound12);
        part_list[part as usize] = part.wrapping_add(bound.wrapping_mul(64));
    }

    let part_count = state.settings.fast_skip_threshold as usize;
    partial_sort_list(&mut part_list, 32, part_count.min(32));
    bc6h_enc_2p_list(state, &part_list, part_count.min(32));
}

fn bc6h_enc_1p(state: &mut Bc6hEncState) {
    let mut ep = [0.0f32; 8];
    block_segment_core(&mut ep, &state.block, -1);

    let mut qep = [0i32; 8];
    ep_quant_dequant_bc6h(state, &mut qep, &mut ep, 1);

    let mut qblock = [0u32; 2];
    let mut err = block_quant(&mut qblock, &state.block, 4, &ep, 0);

    // refine
    for _ in 0..state.settings.refine_iterations_1p {
        opt_endpoints(&mut ep, &state.block, 4, &qblock, -1);
        ep_quant_dequant_bc6h(state, &mut qep, &mut ep, 1);
        err = block_quant(&mut qblock, &state.block, 4, &ep, 0);
    }

    if err < state.best_err {
        state.best_err = err;
        bc6h_code_1p(&mut state.best_data, &mut qep, &mut qblock, state.mode);
    }
}

/// The lowest and highest quantized values an endpoint may take in a mode, from the middle of
/// the block's values and the spread the mode can hold.
fn compute_qbounds(state: &mut Bc6hEncState, rgb_span: &[f32; 3]) {
    let mut bounds = [0.0f32; 8];

    for p in 0..3 {
        let middle = (state.rgb_bounds[p] + state.rgb_bounds[3 + p]) / 2.0;

        bounds[p] = middle - rgb_span[p] / 2.0;
        bounds[4 + p] = middle + rgb_span[p] / 2.0;
    }

    let epb = state.epb;
    ep_quant_bc6h(&mut state.qbounds, &bounds, epb, 1);
}

fn compute_qbounds2(state: &mut Bc6hEncState, span: f32, max_span_idx: i32) {
    let mut rgb_span = [span; 3];
    for p in 0..3 {
        rgb_span[p] *= if p as i32 == max_span_idx { 2.0 } else { 1.0 };
    }
    compute_qbounds(state, &rgb_span);
}

fn bc6h_test_mode(state: &mut Bc6hEncState, mode: usize, enc: bool, margin: f32) {
    let mode_bits = get_mode_bits(mode);
    let span = get_span(mode);
    let max_span = state.max_span;
    let max_span_idx = state.max_span_idx;

    if max_span * margin > span {
        return;
    }

    if mode >= 10 {
        state.epb = mode_bits;
        state.mode = mode as i32;

        compute_qbounds(state, &[span; 3]);
        if enc {
            bc6h_enc_1p(state);
        }
    } else if mode <= 1 || mode == 5 || mode == 9 {
        state.epb = mode_bits;
        state.mode = mode as i32;

        compute_qbounds(state, &[span; 3]);
        if enc {
            bc6h_enc_2p(state);
        }
    } else {
        state.epb = mode_bits;
        state.mode = mode as i32 + max_span_idx;

        compute_qbounds2(state, span, max_span_idx);
        if enc {
            bc6h_enc_2p(state);
        }
    }
}

// BC6H bitstream coding

fn bit_at(v: u32, pos: u32) -> u32 {
    (v >> pos) & 1
}

fn reverse_bits(mut v: u32, bits: u32) -> u32 {
    if bits == 2 {
        (v >> 1) + (v & 1) * 2
    } else {
        // (Six bits.)
        v = (v & 0x5555) * 2 + ((v >> 1) & 0x5555);
        (v >> 4) + ((v >> 2) & 3) * 4 + (v & 3) * 16
    }
}

/// The mode's bits and the endpoints, laid out as the format has them for each mode.
#[rustfmt::skip]
#[allow(clippy::identity_op, clippy::erasing_op)]
fn bc6h_pack(packed: &mut [u32; 4], qep_in: &[i32], mode: i32) {
    let mut qep = [0u32; 16];
    for (value, &given) in qep.iter_mut().zip(qep_in) {
        *value = given as u32;
    }

    if mode == 0 {
        let mut pred_qep = [0u32; 16];
        for p in 0..3 {
            pred_qep[     p] = qep[p];
            pred_qep[ 4 + p] = qep[ 4 + p].wrapping_sub(qep[p]) & 31;
            pred_qep[ 8 + p] = qep[ 8 + p].wrapping_sub(qep[p]) & 31;
            pred_qep[12 + p] = qep[12 + p].wrapping_sub(qep[p]) & 31;
        }


        // g2[4], b2[4], b3[4],
        // r0[9:0],
        // g0[9:0],
        // b0[9:0],
        // r1[4:0], g3[4], g2[3:0],
        // g1[4:0], b3[0], g3[3:0],
        // b1[4:0], b3[1], b2[3:0],
        // r2[4:0], b3[2],
        // r3[4:0], b3[3]
        //

        let mut pqep = [0u32; 10];

        pqep[4] = pred_qep[4] + (pred_qep[ 8 + 1] & 15) * 64;
        pqep[5] = pred_qep[5] + (pred_qep[12 + 1] & 15) * 64;
        pqep[6] = pred_qep[6] + (pred_qep[ 8 + 2] & 15) * 64;

        pqep[4] += bit_at(pred_qep[12 + 1], 4) << 5;
        pqep[5] += bit_at(pred_qep[12 + 2], 0) << 5;
        pqep[6] += bit_at(pred_qep[12 + 2], 1) << 5;

        pqep[8] = pred_qep[ 8] + bit_at(pred_qep[12 + 2], 2) * 32;
        pqep[9] = pred_qep[12] + bit_at(pred_qep[12 + 2], 3) * 32;

        packed[0] = get_mode_prefix(0);
        packed[0] += bit_at(pred_qep[ 8 + 1], 4) << 2;
        packed[0] += bit_at(pred_qep[ 8 + 2], 4) << 3;
        packed[0] += bit_at(pred_qep[12 + 2], 4) << 4;

        packed[1] = (pred_qep[2] << 20) + (pred_qep[1] << 10) + pred_qep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (pqep[9] << 6) + pqep[8];
    } else if mode == 1 {
        let mut pred_qep = [0u32; 16];
        for p in 0..3 {
            pred_qep[     p] = qep[p];
            pred_qep[ 4 + p] = qep[ 4 + p].wrapping_sub(qep[p]) & 63;
            pred_qep[ 8 + p] = qep[ 8 + p].wrapping_sub(qep[p]) & 63;
            pred_qep[12 + p] = qep[12 + p].wrapping_sub(qep[p]) & 63;
        }


        // g2[5], g3[4], g3[5],
        // r0[6:0], b3[0], b3[1], b2[4],
        // g0[6:0], b2[5], b3[2], g2[4],
        // b0[6:0], b3[3], b3[5], b3[4],
        // r1[5:0], g2[3:0],
        // g1[5:0], g3[3:0],
        // b1[5:0], b2[3:0],
        // r2[5:0],
        // r3[5:0]
        //

        let mut pqep = [0u32; 8];

        pqep[0] = pred_qep[0];
        pqep[0] += bit_at(pred_qep[12 + 2], 0) << 7;
        pqep[0] += bit_at(pred_qep[12 + 2], 1) << 8;
        pqep[0] += bit_at(pred_qep[ 8 + 2], 4) << 9;

        pqep[1] = pred_qep[1];
        pqep[1] += bit_at(pred_qep[ 8 + 2], 5) << 7;
        pqep[1] += bit_at(pred_qep[12 + 2], 2) << 8;
        pqep[1] += bit_at(pred_qep[ 8 + 1], 4) << 9;

        pqep[2] = pred_qep[2];
        pqep[2] += bit_at(pred_qep[12 + 2], 3) << 7;
        pqep[2] += bit_at(pred_qep[12 + 2], 5) << 8;
        pqep[2] += bit_at(pred_qep[12 + 2], 4) << 9;

        pqep[4] = pred_qep[4] + (pred_qep[ 8 + 1] & 15) * 64;
        pqep[5] = pred_qep[5] + (pred_qep[12 + 1] & 15) * 64;
        pqep[6] = pred_qep[6] + (pred_qep[ 8 + 2] & 15) * 64;

        packed[0] = get_mode_prefix(1);
        packed[0] += bit_at(pred_qep[ 8 + 1], 5) << 2;
        packed[0] += bit_at(pred_qep[12 + 1], 4) << 3;
        packed[0] += bit_at(pred_qep[12 + 1], 5) << 4;

        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (pred_qep[12] << 6) + pred_qep[8];
    } else if mode == 2 || mode == 3 || mode == 4 {
        // r0[9:0], g0[9:0], b0[9:0],
        // r1[3:0], xx[y], xx[y], g2[3:0],
        // g1[3:0], xx[y], xx[y], g3[3:0],
        // b1[3:0], xx[y], xx[y], b2[3:0],
        // r2[3:0], xx[y], xx[y],
        // r3[3:0], xx[y], xx[y]
        //

        let mut dqep = [0u32; 16];
        for p in 0..3 {
            let mut mask = 15;
            if p as i32 == mode - 2 {
                mask = 31;
            }
            dqep[p] = qep[p];
            dqep[ 4 + p] = qep[ 4 + p].wrapping_sub(qep[p]) & mask;
            dqep[ 8 + p] = qep[ 8 + p].wrapping_sub(qep[p]) & mask;
            dqep[12 + p] = qep[12 + p].wrapping_sub(qep[p]) & mask;
        }


        let mut pqep = [0u32; 10];

        pqep[0] = dqep[0] & 1023;
        pqep[1] = dqep[1] & 1023;
        pqep[2] = dqep[2] & 1023;

        pqep[4] = dqep[4] + (dqep[ 8 + 1] & 15) * 64;
        pqep[5] = dqep[5] + (dqep[12 + 1] & 15) * 64;
        pqep[6] = dqep[6] + (dqep[ 8 + 2] & 15) * 64;

        pqep[8] = dqep[8];
        pqep[9] = dqep[12];

        if mode == 2 {
        // r0[9:0], g0[9:0], b0[9:0],
        // r1[3:0], r1[4],  r0[10], g2[3:0],
        // g1[3:0], g0[10], b3[0],  g3[3:0],
        // b1[3:0], b0[10], b3[1],  b2[3:0],
        // r2[3:0], r2[4],  b3[2],
        // r3[3:0], r3[4],  b3[3]
        //

            packed[0] = get_mode_prefix(2);

        //
            pqep[5] += bit_at(dqep[0 + 1], 10) << 4;
            pqep[6] += bit_at(dqep[0 + 2], 10) << 4;
        //
        //

            pqep[4] += bit_at(dqep[0 + 0], 10) << 5;
            pqep[5] += bit_at(dqep[12 + 2], 0) << 5;
            pqep[6] += bit_at(dqep[12 + 2], 1) << 5;
            pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
            pqep[9] += bit_at(dqep[12 + 2], 3) << 5;
        }
        if mode == 3 {
        // r0[9:0], g0[9:0], b0[9:0],
        // r1[3:0], r0[10], g3[4],  g2[3:0],
        // g1[3:0], g1[4],  g0[10], g3[3:0],
        // b1[3:0], b0[10], b3[1],  b2[3:0],
        // r2[3:0], b3[0],  b3[2],
        // r3[3:0], g2[4],  b3[3]
        //

            packed[0] = get_mode_prefix(3);

            pqep[4] += bit_at(dqep[0 + 0], 10) << 4;
        //
            pqep[6] += bit_at(dqep[0 + 2], 10) << 4;
            pqep[8] += bit_at(dqep[12 + 2], 0) << 4;
            pqep[9] += bit_at(dqep[ 8 + 1], 4) << 4;

            pqep[4] += bit_at(dqep[12 + 1], 4) << 5;
            pqep[5] += bit_at(dqep[0 + 1], 10) << 5;
            pqep[6] += bit_at(dqep[12 + 2], 1) << 5;
            pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
            pqep[9] += bit_at(dqep[12 + 2], 3) << 5;
        }
        if mode == 4 {
        // r0[9:0], g0[9:0], b0[9:0],
        // r1[3:0], r0[10], b2[4],  g2[3:0],
        // g1[3:0], g0[10], b3[0],  g3[3:0],
        // b1[3:0], b1[4],  b0[10], b2[3:0],
        // r2[3:0], b3[1],  b3[2],
        // r3[3:0], b3[4],  b3[3]
        //

            packed[0] = get_mode_prefix(4);

            pqep[4] += bit_at(dqep[0 + 0], 10) << 4;
            pqep[5] += bit_at(dqep[0 + 1], 10) << 4;
        //
            pqep[8] += bit_at(dqep[12 + 2], 1) << 4;
            pqep[9] += bit_at(dqep[12 + 2], 4) << 4;

            pqep[4] += bit_at(dqep[ 8 + 2], 4) << 5;
            pqep[5] += bit_at(dqep[12 + 2], 0) << 5;
            pqep[6] += bit_at(dqep[0 + 2], 10) << 5;
            pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
            pqep[9] += bit_at(dqep[12 + 2], 3) << 5;
        }

        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (pqep[9] << 6) + pqep[8];
    } else if mode == 5 {
        let mut dqep = [0u32; 16];
        for p in 0..3 {
            dqep[p] = qep[p];
            dqep[ 4 + p] = qep[ 4 + p].wrapping_sub(qep[p]) & 31;
            dqep[ 8 + p] = qep[ 8 + p].wrapping_sub(qep[p]) & 31;
            dqep[12 + p] = qep[12 + p].wrapping_sub(qep[p]) & 31;
        }


        // r0[8:0], b2[4],
        // g0[8:0], g2[4],
        // b0[8:0], b3[4],
        // r1[4:0], g3[4], g2[3:0],
        // g1[4:0], b3[0], g3[3:0],
        // b1[4:0], b3[1], b2[3:0],
        // r2[4:0], b3[2],
        // r3[4:0], b3[3]
        //

        let mut pqep = [0u32; 10];

        pqep[0] = dqep[0];
        pqep[1] = dqep[1];
        pqep[2] = dqep[2];
        pqep[4] = dqep[4] + (dqep[ 8 + 1] & 15) * 64;
        pqep[5] = dqep[5] + (dqep[12 + 1] & 15) * 64;
        pqep[6] = dqep[6] + (dqep[ 8 + 2] & 15) * 64;
        pqep[8] = dqep[8];
        pqep[9] = dqep[12];

        pqep[0] += bit_at(dqep[ 8 + 2], 4) << 9;
        pqep[1] += bit_at(dqep[ 8 + 1], 4) << 9;
        pqep[2] += bit_at(dqep[12 + 2], 4) << 9;

        pqep[4] += bit_at(dqep[12 + 1], 4) << 5;
        pqep[5] += bit_at(dqep[12 + 2], 0) << 5;
        pqep[6] += bit_at(dqep[12 + 2], 1) << 5;

        pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
        pqep[9] += bit_at(dqep[12 + 2], 3) << 5;

        packed[0] = get_mode_prefix(5);

        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (pqep[9] << 6) + pqep[8];
    } else if mode == 6 || mode == 7 || mode == 8 {
        // r0[7:0], xx[y], b2[4],
        // g0[7:0], xx[y], g2[4],
        // b0[7:0], xx[y], b3[4],
        // r1[4:0], xx[y], g2[3:0],
        // g1[4:0], xx[y], g3[3:0],
        // b1[4:0], xx[y], b2[3:0],
        // r2[4:0], xx[y],
        // r3[4:0], xx[y]
        //

        let mut dqep = [0u32; 16];
        for p in 0..3 {
            let mut mask = 31;
            if p as i32 == mode - 6 {
                mask = 63;
            }
            dqep[p] = qep[p];
            dqep[ 4 + p] = qep[ 4 + p].wrapping_sub(qep[p]) & mask;
            dqep[ 8 + p] = qep[ 8 + p].wrapping_sub(qep[p]) & mask;
            dqep[12 + p] = qep[12 + p].wrapping_sub(qep[p]) & mask;
        }


        let mut pqep = [0u32; 10];

        pqep[0] = dqep[0];
        pqep[0] += bit_at(dqep[ 8 + 2], 4) << 9;

        pqep[1] = dqep[1];
        pqep[1] += bit_at(dqep[ 8 + 1], 4) << 9;

        pqep[2] = dqep[2];
        pqep[2] += bit_at(dqep[12 + 2], 4) << 9;

        pqep[4] = dqep[4] + (dqep[ 8 + 1] & 15) * 64;
        pqep[5] = dqep[5] + (dqep[12 + 1] & 15) * 64;
        pqep[6] = dqep[6] + (dqep[ 8 + 2] & 15) * 64;

        pqep[8] = dqep[8];
        pqep[9] = dqep[12];

        if mode == 6 {
        // r0[7:0], g3[4], b2[4],
        // g0[7:0], b3[2], g2[4],
        // b0[7:0], b3[3], b3[4],
        // r1[4:0], r1[5], g2[3:0],
        // g1[4:0], b3[0], g3[3:0],
        // b1[4:0], b3[1], b2[3:0],
        // r2[5:0],
        // r3[5:0]
        //

            packed[0] = get_mode_prefix(6);

            pqep[0] += bit_at(dqep[12 + 1], 4) << 8;
            pqep[1] += bit_at(dqep[12 + 2], 2) << 8;
            pqep[2] += bit_at(dqep[12 + 2], 3) << 8;
        //
            pqep[5] += bit_at(dqep[12 + 2], 0) << 5;
            pqep[6] += bit_at(dqep[12 + 2], 1) << 5;
        //
        //
        }
        if mode == 7 {
        // r0[7:0], b3[0], b2[4],
        // g0[7:0], g2[5], g2[4],
        // b0[7:0], g3[5], b3[4],
        // r1[4:0], g3[4], g2[3:0],
        // g1[4:0], g1[5], g3[3:0],
        // b1[4:0], b3[1], b2[3:0],
        // r2[4:0], b3[2],
        // r3[4:0], b3[3]
        //

            packed[0] = get_mode_prefix(7);

            pqep[0] += bit_at(dqep[12 + 2], 0) << 8;
            pqep[1] += bit_at(dqep[ 8 + 1], 5) << 8;
            pqep[2] += bit_at(dqep[12 + 1], 5) << 8;
            pqep[4] += bit_at(dqep[12 + 1], 4) << 5;
        //
            pqep[6] += bit_at(dqep[12 + 2], 1) << 5;
            pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
            pqep[9] += bit_at(dqep[12 + 2], 3) << 5;
        }
        if mode == 8 {
        // r0[7:0], b3[1], b2[4],
        // g0[7:0], b2[5], g2[4],
        // b0[7:0], b3[5], b3[4],
        // r1[4:0], g3[4], g2[3:0],
        // g1[4:0], b3[0], g3[3:0],
        // b1[4:0], b1[5], b2[3:0],
        // r2[4:0], b3[2],
        // r3[4:0], b3[3]
        //

            packed[0] = get_mode_prefix(8);

            pqep[0] += bit_at(dqep[12 + 2], 1) << 8;
            pqep[1] += bit_at(dqep[ 8 + 2], 5) << 8;
            pqep[2] += bit_at(dqep[12 + 2], 5) << 8;
            pqep[4] += bit_at(dqep[12 + 1], 4) << 5;
            pqep[5] += bit_at(dqep[12 + 2], 0) << 5;
        //
            pqep[8] += bit_at(dqep[12 + 2], 2) << 5;
            pqep[9] += bit_at(dqep[12 + 2], 3) << 5;
        }

        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (pqep[9] << 6) + pqep[8];
    } else if mode == 9 {
        // r0[5:0], g3[4], b3[0], b3[1], b2[4], // 10
        // g0[5:0], g2[5], b2[5], b3[2], g2[4], // 10
        // b0[5:0], g3[5], b3[3], b3[5], b3[4], // 10
        // r1[5:0], g2[3:0],  // 10
        // g1[5:0], g3[3:0],  // 10
        // b1[5:0], b2[3:0],  // 10
        // r2[5:0],  // 6
        // r3[5:0]   // 6
        //

        let mut pqep = [0u32; 10];

        pqep[0] = qep[0];
        pqep[0] += bit_at(qep[12 + 1], 4) << 6;
        pqep[0] += bit_at(qep[12 + 2], 0) << 7;
        pqep[0] += bit_at(qep[12 + 2], 1) << 8;
        pqep[0] += bit_at(qep[ 8 + 2], 4) << 9;

        pqep[1] = qep[1];
        pqep[1] += bit_at(qep[ 8 + 1], 5) << 6;
        pqep[1] += bit_at(qep[ 8 + 2], 5) << 7;
        pqep[1] += bit_at(qep[12 + 2], 2) << 8;
        pqep[1] += bit_at(qep[ 8 + 1], 4) << 9;

        pqep[2] = qep[2];
        pqep[2] += bit_at(qep[12 + 1], 5) << 6;
        pqep[2] += bit_at(qep[12 + 2], 3) << 7;
        pqep[2] += bit_at(qep[12 + 2], 5) << 8;
        pqep[2] += bit_at(qep[12 + 2], 4) << 9;

        pqep[4] = qep[4] + (qep[ 8 + 1] & 15) * 64;
        pqep[5] = qep[5] + (qep[12 + 1] & 15) * 64;
        pqep[6] = qep[6] + (qep[ 8 + 2] & 15) * 64;

        packed[0] = get_mode_prefix(9);
        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
        packed[3] = (qep[12] << 6) + qep[8];
    } else if mode == 10 {
        // the only mode with nothing to do ~

        packed[0] = get_mode_prefix(10);
        packed[1] = (qep[2] << 20) + (qep[1] << 10) + qep[0];
        packed[2] = (qep[6] << 20) + (qep[5] << 10) + qep[4];
    } else if mode == 11 {
        let mut dqep = [0u32; 8];
        for p in 0..3 {
            dqep[p] = qep[p];
            dqep[4 + p] = qep[4 + p].wrapping_sub(qep[p]) & 511;
        }


        // r0[9:0], g0[9:0], b0[9:0],
        // r1[8:0], r0[10],
        // g1[8:0], g0[10],
        // b1[8:0], b0[10]
        //

        let mut pqep = [0u32; 8];

        pqep[0] = dqep[0] & 1023;
        pqep[1] = dqep[1] & 1023;
        pqep[2] = dqep[2] & 1023;

        pqep[4] = dqep[4] + (dqep[0] >> 10) * 512;
        pqep[5] = dqep[5] + (dqep[1] >> 10) * 512;
        pqep[6] = dqep[6] + (dqep[2] >> 10) * 512;

        packed[0] = get_mode_prefix(11);
        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
    } else if mode == 12 {
        let mut dqep = [0u32; 8];
        for p in 0..3 {
            dqep[p] = qep[p];
            dqep[4 + p] = qep[4 + p].wrapping_sub(qep[p]) & 255;
        }


        // r0[9:0], g0[9:0], b0[9:0],
        // r1[7:0], r0[10:11],
        // g1[7:0], g0[10:11],
        // b1[7:0], b0[10:11]
        //

        let mut pqep = [0u32; 8];

        pqep[0] = dqep[0] & 1023;
        pqep[1] = dqep[1] & 1023;
        pqep[2] = dqep[2] & 1023;

        pqep[4] = dqep[4] + reverse_bits(dqep[0] >> 10, 2) * 256;
        pqep[5] = dqep[5] + reverse_bits(dqep[1] >> 10, 2) * 256;
        pqep[6] = dqep[6] + reverse_bits(dqep[2] >> 10, 2) * 256;

        packed[0] = get_mode_prefix(12);
        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
    } else if mode == 13 {
        let mut dqep = [0u32; 8];
        for p in 0..3 {
            dqep[p] = qep[p];
            dqep[4 + p] = qep[4 + p].wrapping_sub(qep[p]) & 15;
        }


        // r0[9:0], g0[9:0], b0[9:0],
        // r1[3:0], r0[10:15],
        // g1[3:0], g0[10:15],
        // b1[3:0], b0[10:15]
        //

        let mut pqep = [0u32; 8];

        pqep[0] = dqep[0] & 1023;
        pqep[1] = dqep[1] & 1023;
        pqep[2] = dqep[2] & 1023;

        pqep[4] = dqep[4] + reverse_bits(dqep[0] >> 10, 6) * 16;
        pqep[5] = dqep[5] + reverse_bits(dqep[1] >> 10, 6) * 16;
        pqep[6] = dqep[6] + reverse_bits(dqep[2] >> 10, 6) * 16;

        packed[0] = get_mode_prefix(13);
        packed[1] = (pqep[2] << 20) + (pqep[1] << 10) + pqep[0];
        packed[2] = (pqep[6] << 20) + (pqep[5] << 10) + pqep[4];
    }
}

fn swap_ints(qep: &mut [i32], a: usize, b: usize, n: usize) {
    for i in 0..n {
        qep.swap(a + i, b + i);
    }
}

fn bc7_code_apply_swap_mode456(qep: &mut [i32], channels: usize, qblock: &mut [u32; 2], bits: u32) {
    let levels = 1u32 << bits;
    if (qblock[0] & 15) >= levels / 2 {
        swap_ints(qep, 0, channels, channels);

        for value in qblock.iter_mut() {
            *value = 0x1111_1111u32.wrapping_mul(levels - 1).wrapping_sub(*value);
        }
    }
}

/// Swaps a subset's two ends where its first index would not fit in one bit less, and says
/// which pixels' indices have to be turned round for it. For two subsets and indices of three
/// bits (the original's "mode 1").
fn bc7_code_apply_swap_mode01237(qep: &mut [i32], qblock: &[u32; 2], part_id: i32) -> i32 {
    const PAIRS: usize = 2;
    const LEVELS: u32 = 8;
    let mut flips = 0i32;

    let skips = get_skips(part_id);

    for j in 0..PAIRS {
        let k0 = skips[j] as usize;
        let q = (qblock[k0 >> 3] << (28 - (k0 & 7) * 4)) >> 28;

        if q >= LEVELS / 2 {
            swap_ints(qep, 8 * j, 8 * j + 4, 4);
            flips |= get_pattern_mask(part_id, j);
        }
    }

    flips
}

fn put_bits(data: &mut [u32; 5], pos: &mut usize, bits: usize, v: u32) {
    data[*pos / 32] |= v << (*pos % 32);
    if *pos % 32 + bits > 32 {
        data[*pos / 32 + 1] |= v >> (32 - *pos % 32);
    }
    *pos += bits;
}

/// Takes one bit out of the block's bits at a place, moving what is above it down: the index
/// that is stored a bit short.
fn data_shl_1bit_from(data: &mut [u32; 5], from: i32) {
    if from < 96 {
        let shifted = (data[2] >> 1) | (data[3] << 31);
        let mask = (((1i32 << (from - 64)) - 1) >> 1) as u32;
        data[2] = (mask & data[2]) | (!mask & shifted);
        data[3] = (data[3] >> 1) | (data[4] << 31);
        data[4] >>= 1;
    } else if from < 128 {
        let shifted = (data[3] >> 1) | (data[4] << 31);
        let mask = (((1i32 << (from - 96)) - 1) >> 1) as u32;
        data[3] = (mask & data[3]) | (!mask & shifted);
        data[4] >>= 1;
    }
}

fn bc7_code_qblock(
    data: &mut [u32; 5],
    pos: &mut usize,
    qblock: &[u32; 2],
    bits: usize,
    flips: i32,
) {
    let levels = 1u32 << bits;
    let mut flips_shifted = flips;
    for k1 in 0..2 {
        let mut qbits_shifted = qblock[k1];
        for k2 in 0..8 {
            let mut q = qbits_shifted & 15;
            if flips_shifted & 1 > 0 {
                q = (levels - 1) - q;
            }

            if k1 == 0 && k2 == 0 {
                put_bits(data, pos, bits - 1, q);
            } else {
                put_bits(data, pos, bits, q);
            }
            qbits_shifted >>= 4;
            flips_shifted >>= 1;
        }
    }
}

fn bc6h_code_2p(
    data: &mut [u32; 5],
    qep: &mut [i32],
    qblock: &mut [u32; 2],
    part_id: i32,
    mode: i32,
) {
    const BITS: usize = 3;

    let flips = bc7_code_apply_swap_mode01237(qep, qblock, part_id);

    *data = [0; 5];
    let mut pos = 0usize;

    let mut packed = [0u32; 4];
    bc6h_pack(&mut packed, qep, mode);

    // mode
    put_bits(data, &mut pos, 5, packed[0]);

    // endpoints
    put_bits(data, &mut pos, 30, packed[1]);
    put_bits(data, &mut pos, 30, packed[2]);
    put_bits(data, &mut pos, 12, packed[3]);

    // partition
    put_bits(data, &mut pos, 5, part_id as u32);

    // quantized values
    bc7_code_qblock(data, &mut pos, qblock, BITS, flips);

    // bc7_code_adjust_skip_mode01237, for two subsets and indices of three bits
    let skips = get_skips(part_id);
    data_shl_1bit_from(data, 128 + 1 - (15 - skips[1]) * BITS as i32);
}

fn bc6h_code_1p(data: &mut [u32; 5], qep: &mut [i32], qblock: &mut [u32; 2], mode: i32) {
    bc7_code_apply_swap_mode456(qep, 4, qblock, 4);

    *data = [0; 5];
    let mut pos = 0usize;

    let mut packed = [0u32; 4];
    bc6h_pack(&mut packed, qep, mode);

    // mode
    put_bits(data, &mut pos, 5, packed[0]);

    // endpoints
    put_bits(data, &mut pos, 30, packed[1]);
    put_bits(data, &mut pos, 30, packed[2]);

    // quantized values
    bc7_code_qblock(data, &mut pos, qblock, 4, 0);
}

// BC6H core

fn bc6h_setup(state: &mut Bc6hEncState) {
    for p in 0..3 {
        state.rgb_bounds[p] = 65535.0;
        state.rgb_bounds[3 + p] = 0.0;
    }

    // uf16 conversion, min/max
    for p in 0..3 {
        for k in 0..16 {
            state.block[p * 16 + k] = (state.block[p * 16 + k] / 31.0) * 64.0;

            state.rgb_bounds[p] = min_f(state.rgb_bounds[p], state.block[p * 16 + k]);
            state.rgb_bounds[3 + p] = max_f(state.rgb_bounds[3 + p], state.block[p * 16 + k]);
        }
    }

    state.max_span = 0.0;
    state.max_span_idx = 0;

    for p in 0..3 {
        let rgb_span = state.rgb_bounds[3 + p] - state.rgb_bounds[p];
        if rgb_span > state.max_span {
            state.max_span_idx = p as i32;
            state.max_span = rgb_span;
        }
    }
}

fn compress_block_bc6h_core(state: &mut Bc6hEncState) {
    bc6h_setup(state);

    if state.settings.slow_mode {
        for mode in [0, 1, 2, 5, 6, 9, 10, 11, 12, 13] {
            bc6h_test_mode(state, mode, true, 0.0);
        }
    } else {
        if state.settings.fast_skip_threshold > 0 {
            bc6h_test_mode(state, 9, false, 0.0);
            if state.settings.fast_mode {
                bc6h_test_mode(state, 1, false, 1.0);
            }
            bc6h_test_mode(state, 6, false, 1.0 / 1.2);
            bc6h_test_mode(state, 5, false, 1.0 / 1.2);
            bc6h_test_mode(state, 0, false, 1.0 / 1.2);
            bc6h_test_mode(state, 2, false, 1.0);

            bc6h_enc_2p(state);
            if !state.settings.fast_mode {
                bc6h_test_mode(state, 1, true, 0.0);
            }
        }

        bc6h_test_mode(state, 10, false, 0.0);
        bc6h_test_mode(state, 11, false, 1.0);
        bc6h_test_mode(state, 12, false, 1.0);
        bc6h_test_mode(state, 13, false, 1.0);
        bc6h_enc_1p(state);
    }
}

/// Packs 16 pixels of HDR colour, in rows from the top, into an unsigned BC6H block
/// (`BC6H_UF16`): `CompressBlocksBC6H` for one block.
///
/// The pixels are half floats of zero or above. A half with its sign bit set is not a negative
/// number to this packer but a very large one, as in the original: clamp negative values to
/// zero first.
pub fn compress_block_bc6h(
    block: &mut [u8; 16],
    pixels: &[RgbHalf; 16],
    settings: &Bc6hEncSettings,
) {
    let mut state = Bc6hEncState {
        block: [0.0; 64],
        best_err: f32::INFINITY,
        best_data: [0; 5],
        rgb_bounds: [0.0; 6],
        max_span: 0.0,
        max_span_idx: 0,
        mode: 0,
        epb: 0,
        qbounds: [0; 8],
        settings: *settings,
    };

    // load_block_interleaved_16bit
    for (k, pixel) in pixels.iter().enumerate() {
        for p in 0..3 {
            state.block[16 * p + k] = f32::from(pixel[p]);
        }
    }

    compress_block_bc6h_core(&mut state);

    // store_data
    for (bytes, word) in block
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&state.best_data)
    {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
}

/// Packs many blocks, one for each sixteen pixels. The work is shared out among all the
/// machine's threads; the blocks are the same however many there are.
/// [`compress_blocks_bc6h_on`] takes the number of threads.
///
/// # Panics
///
/// When there is not a block for each sixteen pixels.
pub fn compress_blocks_bc6h(
    blocks: &mut [[u8; 16]],
    pixels: &[[RgbHalf; 16]],
    settings: &Bc6hEncSettings,
) {
    compress_blocks_bc6h_on(0, blocks, pixels, settings);
}

/// [`compress_blocks_bc6h`] on a number of threads: 0 for all the machine has, 1 to stay on the
/// calling thread.
///
/// # Panics
///
/// When there is not a block for each sixteen pixels.
pub fn compress_blocks_bc6h_on(
    threads: u32,
    blocks: &mut [[u8; 16]],
    pixels: &[[RgbHalf; 16]],
    settings: &Bc6hEncSettings,
) {
    assert_eq!(
        blocks.len(),
        pixels.len(),
        "a block for each sixteen pixels"
    );
    crate::parallel::for_each_run(blocks, pixels, threads, |blocks, pixels| {
        for (block, pixels) in blocks.iter_mut().zip(pixels) {
            compress_block_bc6h(block, pixels, settings);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_widen_to_the_floats_they_stand_for() {
        assert_eq!(half_to_float(0x0000), 0.0);
        assert_eq!(half_to_float(0x3c00), 1.0);
        assert_eq!(half_to_float(0xc000), -2.0);
        assert_eq!(half_to_float(0x3800), 0.5);
        assert_eq!(half_to_float(0x7bff), 65504.0);
        // The smallest denormal, infinity, and not a number.
        assert_eq!(half_to_float(0x0001), 2.0f32.powi(-24));
        assert_eq!(half_to_float(0x7c00), f32::INFINITY);
        assert_eq!(half_to_float(0xfc00), f32::NEG_INFINITY);
        assert!(half_to_float(0x7e00).is_nan());
        assert!(half_to_float(0x8000).is_sign_negative());
    }

    #[test]
    fn every_half_survives_the_trip_to_a_float_and_back() {
        for half in 0..=u16::MAX {
            let back = float_to_half(half_to_float(half));
            if half & 0x7c00 == 0x7c00 && half & 0x3ff != 0 {
                // Not a number stays not a number.
                assert!(back & 0x7c00 == 0x7c00 && back & 0x3ff != 0, "{half:#x}");
            } else {
                assert_eq!(back, half, "{half:#x}");
            }
        }
        // Rounding is to nearest, ties to even; too large is infinity; too small is nothing.
        assert_eq!(float_to_half(1.0 + 1.0 / 2048.0), 0x3c00);
        assert_eq!(float_to_half(1.0 + 3.0 / 2048.0), 0x3c02);
        assert_eq!(float_to_half(1.0 + 1.1 / 2048.0), 0x3c01);
        assert_eq!(float_to_half(65520.0), 0x7c00);
        assert_eq!(float_to_half(-1e30), 0xfc00);
        assert_eq!(float_to_half(1e-10), 0);
        assert_eq!(float_to_half(2.0f32.powi(-25)), 0);
        assert_eq!(float_to_half(2.0f32.powi(-25) * 1.5), 1);
    }

    /// Sixteen pixels from one colour to another.
    fn ramp(from: [f32; 3], to: [f32; 3]) -> [RgbHalf; 16] {
        std::array::from_fn(|i| {
            std::array::from_fn(|c| float_to_half(from[c] + (to[c] - from[c]) * i as f32 / 15.0))
        })
    }

    #[test]
    fn packed_pixels_unpack_close_to_what_went_in() {
        let profiles = [
            Bc6hEncSettings::veryfast(),
            Bc6hEncSettings::fast(),
            Bc6hEncSettings::basic(),
            Bc6hEncSettings::slow(),
            Bc6hEncSettings::veryslow(),
        ];
        // (Gentle ramps: the format draws its line between halves as bit patterns, which is
        // close to a line between their logarithms, so a wide ramp of plain values bends away
        // from it.)
        for pixels in [
            ramp([1.0, 0.5, 2.0], [1.4, 0.6, 1.6]),
            ramp([2.0, 2.0, 2.0], [2.5, 2.25, 2.0]),
            ramp([0.01, 0.02, 0.03], [0.012, 0.025, 0.027]),
            ramp([8.0, 1.0, 0.0], [8.0, 1.0, 0.0]),
        ] {
            for settings in &profiles {
                let mut block = [0u8; 16];
                compress_block_bc6h(&mut block, &pixels, settings);
                let unpacked = unpack_bc6h_float(&block, false);
                for (a, b) in pixels.iter().zip(&unpacked) {
                    for c in 0..3 {
                        let a = half_to_float(a[c]);
                        assert!(
                            (a - b[c]).abs() <= 0.01 + a * 0.08,
                            "{a} {} {settings:?}",
                            b[c]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn many_blocks_pack_as_one_at_a_time() {
        // Enough of them for more than one thread to take part.
        let pixels: Vec<[RgbHalf; 16]> = (0..150)
            .map(|i| {
                ramp(
                    [i as f32 * 0.01, 0.5, 1.0],
                    [1.0, 0.75 + i as f32 * 0.02, 0.25],
                )
            })
            .collect();
        let mut blocks = vec![[0u8; 16]; pixels.len()];
        compress_blocks_bc6h(&mut blocks, &pixels, &Bc6hEncSettings::basic());
        let mut on_three = vec![[0u8; 16]; pixels.len()];
        compress_blocks_bc6h_on(3, &mut on_three, &pixels, &Bc6hEncSettings::basic());
        assert_eq!(on_three, blocks);
        for (block, pixels) in blocks.iter().zip(&pixels) {
            let mut one = [0u8; 16];
            compress_block_bc6h(&mut one, pixels, &Bc6hEncSettings::basic());
            assert_eq!(&one, block);
        }
    }

    #[test]
    fn a_block_of_a_reserved_mode_is_black() {
        for bits in [0b10011u8, 0b10111, 0b11011, 0b11111] {
            let mut block = [0xffu8; 16];
            block[0] = 0xe0 | bits;
            assert_eq!(unpack_bc6h_half(&block, false), [[0; 3]; 16]);
            assert_eq!(unpack_bc6h_half(&block, true), [[0; 3]; 16]);
        }
    }

    #[test]
    fn a_block_of_one_endpoint_pair_unpacks_to_a_ramp() {
        // Mode 11 (first bits 00011): two endpoints of ten bits a channel, no partition, sixteen
        // indices of four bits (the first of three). Low endpoint 0, high endpoint all ones.
        let mut bits = 0b00011u128;
        bits |= 0x3fff_ffffu128 << 35;
        // The first index is 0, in three bits; the others count up to 15.
        let mut at = 68;
        for index in 1..16u128 {
            bits |= index << at;
            at += 4;
        }
        assert_eq!(at, 128);
        let pixels = unpack_bc6h_half(&bits.to_le_bytes(), false);
        // From 0 up to the largest half the format gives, 0x7bff (65504.0), the same in each
        // channel, never falling.
        assert_eq!(pixels[0], [0; 3]);
        assert_eq!(pixels[15], [0x7bff; 3]);
        assert!(pixels.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
        assert!(
            pixels
                .iter()
                .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
        );
        assert_eq!(
            unpack_bc6h_float(&bits.to_le_bytes(), false)[15],
            [65504.0; 3]
        );
    }
}
