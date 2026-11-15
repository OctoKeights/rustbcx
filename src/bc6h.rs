//! BC6H, the block format for HDR colour: 4x4 pixels of RGB as half floats, 16 bytes a block.
//!
//! Unpacking is the port of the BC6H part of `bcdec.h` (Sergii Kudlai's
//! [bcdec](https://github.com/iOrange/bcdec), MIT or public domain): [`unpack_bc6h_half`] is
//! `bcdec_bc6h_half` and [`unpack_bc6h_float`] is `bcdec_bc6h_float`, for both kinds of BC6H,
//! the unsigned (`BC6H_UF16`) and the signed (`BC6H_SF16`).

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
        val = if val < 0 { -(((-val) * 31) >> 5) } else { (val * 31) >> 5 }; // scale the magnitude by 31 / 32
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

    let mut bstream = Bitstream { low: u64::from_le_bytes(block[..8].try_into().expect("eight bytes")), high: u64::from_le_bytes(block[8..].try_into().expect("eight bytes")) };

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
                if (i | j) != 0 {
                    0
                } else {
                    128
                }
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
        assert!(pixels.iter().all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]));
        assert_eq!(unpack_bc6h_float(&bits.to_le_bytes(), false)[15], [65504.0; 3]);
    }
}
