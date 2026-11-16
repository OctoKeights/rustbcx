//! The ZFP compression tinyexr adds to the format: a port of `FindZFPCompressionParam`,
//! `CompressZfp` and `DecompressZfp` of tinyexr v3.4.0 (`tinyexr.h`), and of what they use of
//! the zfp library v1.0.1 (<https://github.com/LLNL/zfp>): its stream of bits
//! (`bitstream.inl`), the settings of a stream (`zfp.c`), and its coder of blocks of four by
//! four floats (`template/encode*.c`, `template/decode*.c`, `template/codec*.c`, built for
//! floats in two dimensions, without `ZFP_WITH_DAZ` and with the rounding it has by default).
//!
//! A block of a ZFP image is its floats taken as one plane of four by four blocks for each
//! channel. Each block is stored with the exponent of its largest value, as integers turned
//! by a transform that leaves most of them small, and then one plane of bits at a time, from
//! the top: as many bits as a fixed rate allows, as many planes as a precision says, or down
//! to a tolerance. The rate, the precision or the tolerance is in attributes of the header
//! (`zfpCompressionType`, `zfpCompressionRate`, `zfpCompressionPrecision`,
//! `zfpCompressionTolerance`), which a reader needs.
//!
//! What zfp's reversible mode needs (a tolerance that is not more than nothing below its
//! least exponent) tinyexr's settings do not reach, and it is not ported.

use super::Attribute;

/// `TINYEXR_ZFP_COMPRESSIONTYPE_RATE`: so many bits a value.
pub const ZFP_COMPRESSIONTYPE_RATE: i32 = 0;
/// `TINYEXR_ZFP_COMPRESSIONTYPE_PRECISION`: so many planes of bits.
pub const ZFP_COMPRESSIONTYPE_PRECISION: i32 = 1;
/// `TINYEXR_ZFP_COMPRESSIONTYPE_ACCURACY`: an error no larger than a tolerance.
pub const ZFP_COMPRESSIONTYPE_ACCURACY: i32 = 2;

/// The settings of ZFP compression, as the attributes a header carries them in (which tinyexr
/// reads on both sides: without them, it writes at 2 bits a value, and does not read).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZfpSettings {
    /// So many bits a value (at least 9 a block of 16 are kept).
    Rate(f64),
    /// As many planes of bits as a float has. (tinyexr reads the precision but keeps it where
    /// the rate goes, so that any precision is this one.)
    Precision(i32),
    /// No error larger than the tolerance.
    Accuracy(f64),
}

impl ZfpSettings {
    /// The attributes, to go in a header's `custom_attributes`.
    pub fn attributes(&self) -> Vec<Attribute> {
        let attribute = |name: &str, type_name: &str, value: Vec<u8>| Attribute {
            name: name.to_string(),
            type_name: type_name.to_string(),
            value,
        };
        let (kind, value) = match *self {
            ZfpSettings::Rate(rate) => (
                ZFP_COMPRESSIONTYPE_RATE,
                attribute("zfpCompressionRate", "double", rate.to_le_bytes().to_vec()),
            ),
            ZfpSettings::Precision(precision) => (
                ZFP_COMPRESSIONTYPE_PRECISION,
                attribute(
                    "zfpCompressionPrecision",
                    "int",
                    precision.to_le_bytes().to_vec(),
                ),
            ),
            ZfpSettings::Accuracy(tolerance) => (
                ZFP_COMPRESSIONTYPE_ACCURACY,
                attribute(
                    "zfpCompressionTolerance",
                    "double",
                    tolerance.to_le_bytes().to_vec(),
                ),
            ),
        };
        vec![
            attribute("zfpCompressionType", "uchar", vec![kind as u8]),
            value,
        ]
    }
}

/// `ZFPCompressionParam`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZfpParam {
    rate: f64,
    precision: u32,
    tolerance: f64,
    kind: i32,
}

impl Default for ZfpParam {
    fn default() -> Self {
        ZfpParam {
            rate: 2.0,
            precision: 0,
            tolerance: 0.0,
            kind: ZFP_COMPRESSIONTYPE_RATE,
        }
    }
}

/// `FindZFPCompressionParam`: the settings in `attributes`, or none, with why in `err`.
///
/// (As in the original, the precision of `zfpCompressionPrecision` is put where the rate goes,
/// and so a precision is always that of 64 planes: as many as a float has.)
pub(crate) fn find_zfp_compression_param(
    attributes: &[Attribute],
    err: &mut String,
) -> Option<ZfpParam> {
    let mut param = ZfpParam::default();
    let mut found_type = false;

    for attribute in attributes {
        if attribute.name == "zfpCompressionType" {
            if attribute.value.len() == 1 {
                param.kind = i32::from(attribute.value[0]);
                found_type = true;
                break;
            } else {
                err.push_str("zfpCompressionType attribute must be uchar(1 byte) type.\n");
                return None;
            }
        }
    }

    if !found_type {
        err.push_str("`zfpCompressionType` attribute not found.\n");
        return None;
    }

    let value_of = |name: &str, size: usize| {
        attributes
            .iter()
            .find(|attribute| attribute.name == name && attribute.value.len() == size)
            .map(|attribute| attribute.value.as_slice())
    };
    if param.kind == ZFP_COMPRESSIONTYPE_RATE {
        if let Some(value) = value_of("zfpCompressionRate", 8) {
            param.rate = f64::from_le_bytes(value.try_into().expect("eight bytes"));
            return Some(param);
        }
        err.push_str("`zfpCompressionRate` attribute not found.\n");
    } else if param.kind == ZFP_COMPRESSIONTYPE_PRECISION {
        if let Some(value) = value_of("zfpCompressionPrecision", 4) {
            param.rate = f64::from(i32::from_le_bytes(value.try_into().expect("four bytes")));
            return Some(param);
        }
        err.push_str("`zfpCompressionPrecision` attribute not found.\n");
    } else if param.kind == ZFP_COMPRESSIONTYPE_ACCURACY {
        if let Some(value) = value_of("zfpCompressionTolerance", 8) {
            param.tolerance = f64::from_le_bytes(value.try_into().expect("eight bytes"));
            return Some(param);
        }
        err.push_str("`zfpCompressionTolerance` attribute not found.\n");
    } else {
        err.push_str("Unknown value specified for `zfpCompressionType`.\n");
    }

    None
}

/// The settings tinyexr writes with where the header has none of its own (or not all of them).
pub(crate) fn zfp_param_for_writing(attributes: &[Attribute]) -> ZfpParam {
    find_zfp_compression_param(attributes, &mut String::new()).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// zfp

/// `ZFP_MIN_BITS`, `ZFP_MAX_BITS`, `ZFP_MAX_PREC`, `ZFP_MIN_EXP`.
const ZFP_MIN_BITS: u32 = 1;
const ZFP_MAX_BITS: u32 = 16658;
const ZFP_MAX_PREC: u32 = 64;
const ZFP_MIN_EXP: i32 = -1074;
/// `ZFP_HEADER_MAX_BITS`.
const ZFP_HEADER_MAX_BITS: usize = 148;

/// Of floats: `EBITS`, `EBIAS`, `NBMASK`; and the values of a block of two dimensions.
const EBITS: u32 = 8;
const EBIAS: i32 = (1 << (EBITS - 1)) - 1;
const NBMASK: u32 = 0xaaaa_aaaa;
const BLOCK_SIZE: usize = 16;

/// `perm_2`: coefficients by i + j, then by i² + j².
const PERM_2: [usize; 16] = [0, 1, 4, 5, 2, 8, 6, 9, 3, 12, 10, 7, 13, 11, 14, 15];

/// `zfp_stream`, but its stream of bits.
#[derive(Clone, Copy)]
struct ZfpStream {
    minbits: u32,
    maxbits: u32,
    maxprec: u32,
    minexp: i32,
}

impl ZfpStream {
    /// `zfp_stream_open`.
    fn open() -> Self {
        ZfpStream {
            minbits: ZFP_MIN_BITS,
            maxbits: ZFP_MAX_BITS,
            maxprec: ZFP_MAX_PREC,
            minexp: ZFP_MIN_EXP,
        }
    }

    /// `zfp_stream_set_rate` for floats in two dimensions, without alignment for writing at
    /// random.
    fn set_rate(&mut self, rate: f64) {
        let n = 16u32;
        // (`(uint)floor(n * rate + 0.5)`, which for what is not a number, or does not fit,
        // gives what the processor gives: the 64-bit conversion's low 32 bits.)
        let bits = double_to_uint((f64::from(n) * rate + 0.5).floor()).max(1 + 8);
        self.minbits = bits;
        self.maxbits = bits;
        self.maxprec = ZFP_MAX_PREC;
        self.minexp = ZFP_MIN_EXP;
    }

    /// `zfp_stream_set_precision`.
    fn set_precision(&mut self, precision: u32) {
        self.minbits = ZFP_MIN_BITS;
        self.maxbits = ZFP_MAX_BITS;
        self.maxprec = if precision != 0 {
            precision.min(ZFP_MAX_PREC)
        } else {
            ZFP_MAX_PREC
        };
        self.minexp = ZFP_MIN_EXP;
    }

    /// `zfp_stream_set_accuracy`.
    fn set_accuracy(&mut self, tolerance: f64) {
        let mut emin = ZFP_MIN_EXP;
        if tolerance > 0.0 {
            // tolerance = x * 2^emin, with 0.5 <= x < 1
            emin = frexp_exponent(tolerance);
            emin -= 1;
        }
        self.minbits = ZFP_MIN_BITS;
        self.maxbits = ZFP_MAX_BITS;
        self.maxprec = ZFP_MAX_PREC;
        self.minexp = emin;
    }

    /// The settings as tinyexr makes them, or none for a type of settings it does not know.
    fn with(param: &ZfpParam) -> Option<Self> {
        let mut zfp = ZfpStream::open();
        if param.kind == ZFP_COMPRESSIONTYPE_RATE {
            zfp.set_rate(param.rate);
        } else if param.kind == ZFP_COMPRESSIONTYPE_PRECISION {
            zfp.set_precision(param.precision);
        } else if param.kind == ZFP_COMPRESSIONTYPE_ACCURACY {
            zfp.set_accuracy(param.tolerance);
        } else {
            return None;
        }
        Some(zfp)
    }

    /// `zfp_stream_maximum_size` of a field of floats, `nx` by `ny`: the bytes the stream of
    /// it can take, a multiple of eight.
    fn maximum_size(&self, nx: usize, ny: usize) -> usize {
        let blocks = nx.div_ceil(4) * ny.div_ceil(4);
        let values = 16u32;
        // (1 + 8 for a float's exponent.)
        let mut maxbits = 1 + 8;
        maxbits += values - 1 + values * self.maxprec.min(32);
        maxbits = maxbits.min(self.maxbits);
        maxbits = maxbits.max(self.minbits);
        (ZFP_HEADER_MAX_BITS
            .wrapping_add(blocks.wrapping_mul(maxbits as usize))
            .wrapping_add(63)
            & !63)
            / 8
    }
}

/// `(uint)` of a double as x86-64 converts it: through a 64-bit integer, of which the low 32
/// bits are kept; what is not a number, or is out of that integer's reach, gives its least.
fn double_to_uint(value: f64) -> u32 {
    let wide = if value.is_nan()
        || value >= 9_223_372_036_854_775_808.0
        || value < -9_223_372_036_854_775_808.0
    {
        i64::MIN
    } else {
        value as i64
    };
    wide as u32
}

/// `(int32)` of a float as x86-64 converts it: what is not a number, or does not fit, gives
/// the least integer.
fn float_to_int(value: f32) -> i32 {
    if value.is_nan() || value >= 2_147_483_648.0 || value < -2_147_483_648.0 {
        i32::MIN
    } else {
        value as i32
    }
}

/// The exponent `frexp` gives a positive double (the C library's: 0 for an infinity).
fn frexp_exponent(value: f64) -> i32 {
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0x7ff {
        0
    } else if exponent == 0 {
        let mantissa = bits & ((1u64 << 52) - 1);
        if mantissa == 0 {
            0
        } else {
            // A subnormal: as 0.5 times two to one more than the top bit's place.
            -1022 - (mantissa.leading_zeros() as i32 - 12) + 1 - 1
        }
    } else {
        exponent - 1022
    }
}

/// `ldexpf(1, k)`: two to `k` as a float, rounded as the C library rounds it (to infinity
/// above, to nothing below the least subnormal, the half of which rounds to even, nothing).
fn power_of_two(k: i32) -> f32 {
    if k > 127 {
        f32::INFINITY
    } else if k >= -126 {
        f32::from_bits(((k + 127) as u32) << 23)
    } else if k >= -149 {
        f32::from_bits(1u32 << (k + 149))
    } else {
        0.0
    }
}

/// The bits of a stream, in words of 64 bits (`bitstream.inl`, with its default word).
struct BitStream {
    /// Buffered bits, from 0 to 63.
    bits: u32,
    buffer: u64,
    /// The next word to read or write.
    ptr: usize,
    words: Vec<u64>,
}

impl BitStream {
    /// `stream_open` and `stream_rewind`, over `words`.
    fn new(words: Vec<u64>) -> Self {
        BitStream {
            bits: 0,
            buffer: 0,
            ptr: 0,
            words,
        }
    }

    fn read_word(&mut self) -> u64 {
        // (Every read is within the stream: `maximum_size` has room for all a block reads,
        // whatever its bits say.)
        let word = self.words.get(self.ptr).copied().unwrap_or(0);
        self.ptr += 1;
        word
    }

    fn write_word(&mut self, value: u64) {
        self.words[self.ptr] = value;
        self.ptr += 1;
    }

    /// `stream_read_bit`.
    fn read_bit(&mut self) -> u32 {
        if self.bits == 0 {
            self.buffer = self.read_word();
            self.bits = 64;
        }
        self.bits -= 1;
        let bit = (self.buffer & 1) as u32;
        self.buffer >>= 1;
        bit
    }

    /// `stream_write_bit`.
    fn write_bit(&mut self, bit: u32) -> u32 {
        self.buffer = self.buffer.wrapping_add(u64::from(bit) << self.bits);
        self.bits += 1;
        if self.bits == 64 {
            let buffer = self.buffer;
            self.write_word(buffer);
            self.buffer = 0;
            self.bits = 0;
        }
        bit
    }

    /// `stream_read_bits`: `n` bits, 0 to 64 of them, the first the least significant.
    fn read_bits(&mut self, n: u32) -> u64 {
        let mut value = self.buffer;
        if self.bits < n {
            // keep fetching wsize bits until enough bits are buffered (one word does it)
            self.buffer = self.read_word();
            value = value.wrapping_add(self.buffer << self.bits);
            self.bits += 64;
            self.bits -= n;
            if self.bits == 0 {
                // value holds exactly n bits; no need for masking
                self.buffer = 0;
            } else {
                self.buffer >>= 64 - self.bits;
                value &= (2u64 << (n - 1)).wrapping_sub(1);
            }
        } else {
            self.bits -= n;
            // (A shift by the whole word, for 64 bits of a buffer of 64, is not made.)
            self.buffer = if n < 64 { self.buffer >> n } else { 0 };
            value &= if n < 64 { (1u64 << n) - 1 } else { u64::MAX };
        }
        value
    }

    /// `stream_write_bits`: the `n` least significant bits of `value`, 0 to 64 of them; what
    /// was not written comes back, shifted down.
    fn write_bits(&mut self, mut value: u64, mut n: u32) -> u64 {
        // append bit string to buffer
        self.buffer = self.buffer.wrapping_add(value << self.bits);
        self.bits += n;
        // is buffer full?
        if self.bits >= 64 {
            // 1 <= n <= 64; decrement n to ensure valid right shifts below
            value >>= 1;
            n -= 1;
            // output wsize bits while buffer is full
            self.bits -= 64;
            let buffer = self.buffer;
            self.write_word(buffer);
            self.buffer = value >> (n - self.bits);
        }
        self.buffer &= (1u64 << self.bits) - 1;
        value >> n
    }

    /// `stream_rtell`.
    fn rtell(&self) -> u64 {
        self.ptr as u64 * 64 - u64::from(self.bits)
    }

    /// `stream_wtell`.
    fn wtell(&self) -> u64 {
        self.ptr as u64 * 64 + u64::from(self.bits)
    }

    /// `stream_rseek`.
    fn rseek(&mut self, offset: u64) {
        let n = (offset % 64) as u32;
        self.ptr = (offset / 64) as usize;
        if n != 0 {
            self.buffer = self.read_word() >> n;
            self.bits = 64 - n;
        } else {
            self.buffer = 0;
            self.bits = 0;
        }
    }

    /// `stream_skip`.
    fn skip(&mut self, n: u32) {
        let offset = self.rtell() + u64::from(n);
        self.rseek(offset);
    }

    /// `stream_pad`.
    fn pad(&mut self, n: u32) {
        let mut bits = u64::from(self.bits) + u64::from(n);
        while bits >= 64 {
            let buffer = self.buffer;
            self.write_word(buffer);
            self.buffer = 0;
            bits -= 64;
        }
        self.bits = bits as u32;
    }

    /// `stream_flush`.
    fn flush(&mut self) {
        let bits = (64 - self.bits) % 64;
        if bits != 0 {
            self.pad(bits);
        }
    }
}

/// `precision`: the planes of bits to code of a block with the exponent `maxexp`.
fn precision(maxexp: i32, maxprec: u32, minexp: i32) -> u32 {
    maxprec.min((maxexp - minexp + 2 * 2 + 2).max(0) as u32)
}

/// `with_maxbits`: whether the most a block can take is more than `maxbits`.
fn with_maxbits(maxbits: u32, maxprec: u32, size: u32) -> bool {
    (maxprec + 1) * size - 1 > maxbits
}

/// `exponent`: the exponent of a value of no less than nothing, -127 for nothing.
fn exponent(x: f32) -> i32 {
    // use e = -EBIAS when x = 0
    let mut e = -EBIAS;
    if x > 0.0 {
        let bits = x.to_bits();
        let biased = (bits >> 23) as i32;
        // (frexpf: 0 for an infinity; a subnormal's own, which is below the clamp.)
        e = if biased == 0xff {
            0
        } else if biased == 0 {
            -126
        } else {
            biased - 126
        };
        // clamp exponent in case x is subnormal; may still result in overflow
        e = e.max(1 - EBIAS);
    }
    e
}

/// `exponent_block`: the exponent of the largest value of a block (what is not a number left
/// out).
fn exponent_block(block: &[f32; BLOCK_SIZE]) -> i32 {
    let mut max = 0f32;
    for &value in block {
        let f = value.abs();
        if max < f {
            max = f;
        }
    }
    exponent(max)
}

/// `fwd_lift`: the forward transform of the four values from `at`, `s` apart.
fn fwd_lift(p: &mut [i32; BLOCK_SIZE], at: usize, s: usize) {
    let (mut x, mut y, mut z, mut w) = (p[at], p[at + s], p[at + 2 * s], p[at + 3 * s]);
    x = x.wrapping_add(w);
    x >>= 1;
    w = w.wrapping_sub(x);
    z = z.wrapping_add(y);
    z >>= 1;
    y = y.wrapping_sub(z);
    x = x.wrapping_add(z);
    x >>= 1;
    z = z.wrapping_sub(x);
    w = w.wrapping_add(y);
    w >>= 1;
    y = y.wrapping_sub(w);
    w = w.wrapping_add(y >> 1);
    y = y.wrapping_sub(w >> 1);
    p[at] = x;
    p[at + s] = y;
    p[at + 2 * s] = z;
    p[at + 3 * s] = w;
}

/// `inv_lift`.
fn inv_lift(p: &mut [i32; BLOCK_SIZE], at: usize, s: usize) {
    let (mut x, mut y, mut z, mut w) = (p[at], p[at + s], p[at + 2 * s], p[at + 3 * s]);
    y = y.wrapping_add(w >> 1);
    w = w.wrapping_sub(y >> 1);
    y = y.wrapping_add(w);
    w = w.wrapping_shl(1);
    w = w.wrapping_sub(y);
    z = z.wrapping_add(x);
    x = x.wrapping_shl(1);
    x = x.wrapping_sub(z);
    y = y.wrapping_add(z);
    z = z.wrapping_shl(1);
    z = z.wrapping_sub(y);
    w = w.wrapping_add(x);
    x = x.wrapping_shl(1);
    x = x.wrapping_sub(w);
    p[at] = x;
    p[at + s] = y;
    p[at + 2 * s] = z;
    p[at + 3 * s] = w;
}

/// `encode_few_ints`: the planes of bits of up to 64 integers, no more than `maxbits` bits.
fn encode_few_ints(s: &mut BitStream, maxbits: u32, maxprec: u32, data: &[u32; BLOCK_SIZE]) -> u32 {
    let size = BLOCK_SIZE as u32;
    let intprec = 32u32;
    let kmin = intprec.saturating_sub(maxprec);
    let mut bits = maxbits;
    let (mut k, mut n) = (intprec, 0u32);

    // encode one bit plane at a time from MSB to LSB
    while bits != 0 && k > kmin {
        k -= 1;
        // step 1: extract bit plane #k to x
        let mut x = 0u64;
        for (i, &value) in data.iter().enumerate() {
            x += u64::from((value >> k) & 1) << i;
        }
        // step 2: encode first n bits of bit plane
        let m = n.min(bits);
        bits -= m;
        x = s.write_bits(x, m);
        // step 3: unary run-length encode remainder of bit plane
        while bits != 0 && n < size {
            bits -= 1;
            if s.write_bit(u32::from(x != 0)) != 0 {
                // positive group test (x != 0); scan for one-bit
                while bits != 0 && n < size - 1 {
                    bits -= 1;
                    if s.write_bit((x & 1) as u32) != 0 {
                        break;
                    }
                    x >>= 1;
                    n += 1;
                }
            } else {
                // negative group test (x == 0); done with bit plane
                break;
            }
            x >>= 1;
            n += 1;
        }
    }

    maxbits - bits
}

/// `encode_few_ints_prec`: the planes of bits of up to 64 integers, without a limit.
fn encode_few_ints_prec(s: &mut BitStream, maxprec: u32, data: &[u32; BLOCK_SIZE]) -> u32 {
    let offset = s.wtell();
    let size = BLOCK_SIZE as u32;
    let intprec = 32u32;
    let kmin = intprec.saturating_sub(maxprec);
    let (mut k, mut n) = (intprec, 0u32);

    // encode one bit plane at a time from MSB to LSB
    while k > kmin {
        k -= 1;
        // step 1: extract bit plane #k to x
        let mut x = 0u64;
        for (i, &value) in data.iter().enumerate() {
            x += u64::from((value >> k) & 1) << i;
        }
        // step 2: encode first n bits of bit plane
        x = s.write_bits(x, n);
        // step 3: unary run-length encode remainder of bit plane
        while n < size && s.write_bit(u32::from(x != 0)) != 0 {
            while n < size - 1 && s.write_bit((x & 1) as u32) == 0 {
                x >>= 1;
                n += 1;
            }
            x >>= 1;
            n += 1;
        }
    }

    (s.wtell() - offset) as u32
}

/// `decode_few_ints`.
fn decode_few_ints(
    s: &mut BitStream,
    maxbits: u32,
    maxprec: u32,
    data: &mut [u32; BLOCK_SIZE],
) -> u32 {
    let size = BLOCK_SIZE as u32;
    let intprec = 32u32;
    let kmin = intprec.saturating_sub(maxprec);
    let mut bits = maxbits;
    let (mut k, mut n) = (intprec, 0u32);

    // initialize data array to all zeros
    *data = [0; BLOCK_SIZE];

    // decode one bit plane at a time from MSB to LSB
    while bits != 0 && k > kmin {
        k -= 1;
        // step 1: decode first n bits of bit plane #k
        let m = n.min(bits);
        bits -= m;
        let mut x = s.read_bits(m);
        // step 2: unary run-length decode remainder of bit plane
        while bits != 0 && n < size {
            bits -= 1;
            if s.read_bit() != 0 {
                // positive group test; scan for next one-bit
                while bits != 0 && n < size - 1 {
                    bits -= 1;
                    if s.read_bit() != 0 {
                        break;
                    }
                    n += 1;
                }
                // set bit and continue decoding bit plane
                x += 1u64 << n;
            } else {
                // negative group test; done with bit plane
                break;
            }
            n += 1;
        }
        // step 3: deposit bit plane from x
        let mut i = 0;
        while x != 0 {
            data[i] = data[i].wrapping_add(((x & 1) as u32) << k);
            i += 1;
            x >>= 1;
        }
    }

    maxbits - bits
}

/// `decode_few_ints_prec`.
fn decode_few_ints_prec(s: &mut BitStream, maxprec: u32, data: &mut [u32; BLOCK_SIZE]) -> u32 {
    let offset = s.rtell();
    let size = BLOCK_SIZE as u32;
    let intprec = 32u32;
    let kmin = intprec.saturating_sub(maxprec);
    let (mut k, mut n) = (intprec, 0u32);

    // initialize data array to all zeros
    *data = [0; BLOCK_SIZE];

    // decode one bit plane at a time from MSB to LSB
    while k > kmin {
        k -= 1;
        // step 1: decode first n bits of bit plane #k
        let mut x = s.read_bits(n);
        // step 2: unary run-length decode remainder of bit plane
        while n < size && s.read_bit() != 0 {
            while n < size - 1 && s.read_bit() == 0 {
                n += 1;
            }
            x += 1u64 << n;
            n += 1;
        }
        // step 3: deposit bit plane from x
        let mut i = 0;
        while x != 0 {
            data[i] = data[i].wrapping_add(((x & 1) as u32) << k);
            i += 1;
            x >>= 1;
        }
    }

    (s.rtell() - offset) as u32
}

/// `encode_block` of integers: transformed, reordered, and their planes of bits, with at
/// least `minbits` bits.
fn encode_int_block(
    s: &mut BitStream,
    minbits: u32,
    maxbits: u32,
    maxprec: u32,
    iblock: &mut [i32; BLOCK_SIZE],
) -> u32 {
    // perform decorrelating transform: along x, then along y
    for y in 0..4 {
        fwd_lift(iblock, 4 * y, 1);
    }
    for x in 0..4 {
        fwd_lift(iblock, x, 4);
    }
    // reorder signed coefficients and convert to unsigned integer
    let mut ublock = [0u32; BLOCK_SIZE];
    for (u, &at) in ublock.iter_mut().zip(PERM_2.iter()) {
        *u = (iblock[at] as u32).wrapping_add(NBMASK) ^ NBMASK;
    }
    // encode integer coefficients
    let mut bits = if with_maxbits(maxbits, maxprec, BLOCK_SIZE as u32) {
        encode_few_ints(s, maxbits, maxprec, &ublock)
    } else {
        encode_few_ints_prec(s, maxprec, &ublock)
    };
    // write at least minbits bits by padding with zeros
    if bits < minbits {
        s.pad(minbits - bits);
        bits = minbits;
    }
    bits
}

/// `decode_block` of integers.
fn decode_int_block(
    s: &mut BitStream,
    minbits: u32,
    maxbits: u32,
    maxprec: u32,
    iblock: &mut [i32; BLOCK_SIZE],
) -> u32 {
    let mut ublock = [0u32; BLOCK_SIZE];
    // decode integer coefficients
    let mut bits = if with_maxbits(maxbits, maxprec, BLOCK_SIZE as u32) {
        decode_few_ints(s, maxbits, maxprec, &mut ublock)
    } else {
        decode_few_ints_prec(s, maxprec, &mut ublock)
    };
    // read at least minbits bits
    if bits < minbits {
        s.skip(minbits - bits);
        bits = minbits;
    }
    // reorder unsigned coefficients and convert to signed integer
    for (&u, &at) in ublock.iter().zip(PERM_2.iter()) {
        iblock[at] = (u ^ NBMASK).wrapping_sub(NBMASK) as i32;
    }
    // perform decorrelating transform: along y, then along x
    for x in 0..4 {
        inv_lift(iblock, x, 4);
    }
    for y in 0..4 {
        inv_lift(iblock, 4 * y, 1);
    }
    bits
}

/// `zfp_encode_block_float_2`.
fn encode_block(zfp: &ZfpStream, s: &mut BitStream, fblock: &[f32; BLOCK_SIZE]) -> u32 {
    let mut bits = 1u32;
    // compute maximum exponent
    let emax = exponent_block(fblock);
    let maxprec = precision(emax, zfp.maxprec, zfp.minexp);
    let e = if maxprec != 0 {
        (emax + EBIAS) as u32
    } else {
        0
    };
    // encode block only if biased exponent is nonzero
    if e != 0 {
        // encode common exponent; LSB indicates that exponent is nonzero
        bits += EBITS;
        s.write_bits(u64::from(2 * e + 1), bits);
        // perform forward block-floating-point transform
        let scale = power_of_two(30 - emax);
        let mut iblock = [0i32; BLOCK_SIZE];
        for (i, &f) in iblock.iter_mut().zip(fblock.iter()) {
            *i = float_to_int(scale * f);
        }
        // encode integer block
        bits += encode_int_block(
            s,
            zfp.minbits - bits.min(zfp.minbits),
            zfp.maxbits - bits,
            maxprec,
            &mut iblock,
        );
    } else {
        // write single zero-bit to indicate that all values are zero
        s.write_bit(0);
        if zfp.minbits > bits {
            s.pad(zfp.minbits - bits);
            bits = zfp.minbits;
        }
    }
    bits
}

/// `zfp_decode_block_float_2`.
fn decode_block(zfp: &ZfpStream, s: &mut BitStream, fblock: &mut [f32; BLOCK_SIZE]) -> u32 {
    let mut bits = 1u32;
    // test if block has nonzero values
    if s.read_bit() != 0 {
        // decode common exponent
        bits += EBITS;
        let emax = s.read_bits(EBITS) as i32 - EBIAS;
        let maxprec = precision(emax, zfp.maxprec, zfp.minexp);
        // decode integer block
        let mut iblock = [0i32; BLOCK_SIZE];
        bits += decode_int_block(
            s,
            zfp.minbits - bits.min(zfp.minbits),
            zfp.maxbits - bits,
            maxprec,
            &mut iblock,
        );
        // perform inverse block-floating-point transform
        let scale = power_of_two(emax - 30);
        for (f, &i) in fblock.iter_mut().zip(iblock.iter()) {
            *f = scale * i as f32;
        }
    } else {
        // set all values to zero
        *fblock = [0.0; BLOCK_SIZE];
        if zfp.minbits > bits {
            s.skip(zfp.minbits - bits);
            bits = zfp.minbits;
        }
    }
    bits
}

/// The most the port gives a stream (tinyexr allocates whatever the settings say).
const MAX_STREAM: usize = 1 << 30;

/// The bytes the stream of a block of `width` by `lines` pixels of `num_channels` channels is
/// given, or none where that is more than a gigabyte.
pub(crate) fn zfp_stream_size(
    param: &ZfpParam,
    width: i32,
    lines: i32,
    num_channels: usize,
) -> Option<usize> {
    let zfp = ZfpStream::with(param)?;
    let size = zfp.maximum_size(
        width.max(0) as usize,
        (lines as u32).wrapping_mul(num_channels as u32) as usize,
    );
    (size <= MAX_STREAM).then_some(size)
}

/// Why `compress_zfp` does not compress a block.
pub(crate) enum ZfpRefusal {
    /// Its width or its lines are not a multiple of four, which the original does not look at
    /// past its own refusal (it then writes a length it never set).
    NotMultipleOfFour,
    /// Its settings ask for more than a gigabyte.
    TooLarge,
}

/// `CompressZfp`: `num_lines` lines of `width` floats of each of `num_channels` channels, one
/// plane after the other (as tinyexr takes the block it hands over, whose lines are in fact
/// the channels of one line after the other), compressed.
pub(crate) fn compress_zfp(
    input: &[u8],
    width: i32,
    num_lines: i32,
    num_channels: usize,
    param: &ZfpParam,
) -> Result<Vec<u8>, ZfpRefusal> {
    if width % 4 != 0 || num_lines % 4 != 0 {
        return Err(ZfpRefusal::NotMultipleOfFour);
    }
    // (`with` refuses only a type of settings that `zfp_param_for_writing` does not give.)
    let zfp = ZfpStream::with(param).ok_or(ZfpRefusal::NotMultipleOfFour)?;
    let (width, num_lines) = (width as usize, num_lines as usize);

    let buf_size = zfp.maximum_size(
        width,
        (num_lines as u32).wrapping_mul(num_channels as u32) as usize,
    );
    if buf_size > MAX_STREAM {
        return Err(ZfpRefusal::TooLarge);
    }
    let mut stream = BitStream::new(vec![0u64; buf_size / 8]);

    let image_size = width * num_lines;
    let float_at = |index: usize| {
        f32::from_bits(u32::from_le_bytes(
            input[4 * index..4 * index + 4]
                .try_into()
                .expect("four bytes"),
        ))
    };
    for c in 0..num_channels {
        // compress 4x4 pixel block.
        for y in (0..num_lines).step_by(4) {
            for x in (0..width).step_by(4) {
                let mut fblock = [0f32; BLOCK_SIZE];
                for j in 0..4 {
                    for i in 0..4 {
                        fblock[j * 4 + i] = float_at(c * image_size + ((y + j) * width + (x + i)));
                    }
                }
                encode_block(&zfp, &mut stream, &fblock);
            }
        }
    }

    stream.flush();
    let size = stream.ptr * 8;
    let mut out = Vec::with_capacity(size);
    for word in &stream.words[..stream.ptr] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    Ok(out)
}

/// `DecompressZfp`: `src` into `dst`, `dst_num_lines` lines of `dst_width` floats of each of
/// `num_channels` channels, one plane after the other. The answer, which the original's
/// caller does not look at, is whether it was done; none where the settings ask for more
/// than a gigabyte, which stops the original where it can have no more.
///
/// The original takes a block of as many bytes as the block has values for one that is not
/// compressed, copies it, and then goes on; and writes past `dst` for a block with a channel
/// that is not of floats (which then is not read), and past the buffer it reads from for a
/// block longer than the most its settings make. Here what is past them is left out.
pub(crate) fn decompress_zfp(
    dst: &mut [u8],
    dst_width: i32,
    dst_num_lines: i32,
    num_channels: usize,
    src: &[u8],
    param: &ZfpParam,
) -> Option<bool> {
    let uncompressed_size = (dst_width as usize)
        .wrapping_mul(dst_num_lines as usize)
        .wrapping_mul(num_channels);

    if uncompressed_size == src.len() {
        // Data is not compressed(Issue 40).
        dst[..src.len()].copy_from_slice(src);
    }

    if dst_width % 4 != 0 || dst_num_lines % 4 != 0 {
        return Some(false);
    }

    let Some(zfp) = ZfpStream::with(param) else {
        return Some(false);
    };
    let (width, num_lines) = (dst_width as usize, dst_num_lines as usize);

    let buf_size = zfp.maximum_size(
        width,
        (num_lines as u32).wrapping_mul(num_channels as u32) as usize,
    );
    if buf_size > MAX_STREAM {
        return None;
    }
    let mut words = vec![0u64; buf_size / 8];
    for (word, bytes) in words.iter_mut().zip(src.chunks(8)) {
        let mut eight = [0u8; 8];
        eight[..bytes.len()].copy_from_slice(bytes);
        *word = u64::from_le_bytes(eight);
    }
    let mut stream = BitStream::new(words);

    let image_size = width * num_lines;
    for c in 0..num_channels {
        // decompress 4x4 pixel block.
        for y in (0..num_lines).step_by(4) {
            for x in (0..width).step_by(4) {
                let mut fblock = [0f32; BLOCK_SIZE];
                decode_block(&zfp, &mut stream, &mut fblock);
                for j in 0..4 {
                    for i in 0..4 {
                        let at = 4 * (c * image_size + ((y + j) * width + (x + i)));
                        if let Some(slot) = dst.get_mut(at..at + 4) {
                            slot.copy_from_slice(&fblock[j * 4 + i].to_bits().to_le_bytes());
                        }
                    }
                }
            }
        }
    }

    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frexp_and_powers_of_two() {
        for value in [1.0f64, 0.75, 3.0, 1e-300, 5e-324, 1e-310, 1e300] {
            let e = frexp_exponent(value);
            let mantissa = value / 2f64.powi(e);
            // (powi of very small exponents loses the value; compare on the bits instead.)
            if value > 1e-290 {
                assert!((0.5..1.0).contains(&mantissa), "{value}: {e}");
            }
        }
        assert_eq!(frexp_exponent(5e-324), -1073);
        assert_eq!(frexp_exponent(f64::INFINITY), 0);
        assert_eq!(frexp_exponent(f64::MAX), 1024);
        assert_eq!(power_of_two(0), 1.0);
        assert_eq!(power_of_two(-149), f32::from_bits(1));
        assert_eq!(power_of_two(-150), 0.0);
        assert_eq!(power_of_two(128), f32::INFINITY);
    }

    #[test]
    fn blocks_come_back() {
        // At full precision, smooth values come back nearly as they were.
        let param = ZfpParam {
            kind: ZFP_COMPRESSIONTYPE_PRECISION,
            ..ZfpParam::default()
        };
        let (width, lines, channels) = (8, 4, 2);
        let mut input = Vec::new();
        let mut values = Vec::new();
        for k in 0..width * lines * channels {
            let value = (k as f32 * 0.37).sin() * 10.0;
            values.push(value);
            input.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        let Ok(packed) = compress_zfp(&input, width as i32, lines as i32, channels, &param) else {
            panic!("not compressed")
        };
        let mut out = vec![0u8; input.len()];
        assert_eq!(
            decompress_zfp(
                &mut out,
                width as i32,
                lines as i32,
                channels,
                &packed,
                &param
            ),
            Some(true)
        );
        for (k, &value) in values.iter().enumerate() {
            let got = f32::from_bits(u32::from_le_bytes(
                out[4 * k..4 * k + 4].try_into().unwrap(),
            ));
            assert!((got - value).abs() < 1e-4, "{k}: {got} {value}");
        }
    }
}
