//! DWAA and DWAB, DreamWorks Animation's lossy compressions for `.exr` files, a block at a
//! time: a port of OpenEXRCore's (`internal_dwa*.{c,h}` of OpenEXR v3.5.4). [`compress`]
//! makes a block of an image's lines into what a file holds, and [`uncompress`] makes it
//! back, or gives OpenEXRCore's code for why not ([`CORRUPT_CHUNK`] for most of what is
//! damaged). [`crate::tinyexr`] reads and writes files with them.
//!
//! A block is its lines one after the other, each with its channels one after the other (in
//! the order of their names, as a file has them), as little-endian halves, floats and 32-bit
//! integers. A channel's name decides what is done with it: the red, green and blue of a
//! layer (`R`, `G` and `B`, or `diffuse.R` and so on) are converted to Y'CbCr together and
//! coded by a lossy DCT, as is a `Y`, `BY` or `RY`; an `A` is kept whole by RLE; anything else
//! is kept whole by ZIP. DWAA's blocks are of 32 lines, DWAB's of 256; they are otherwise the
//! same, but that OpenEXR compresses the AC coefficients of a tile of DWAA with ZIP and every
//! other block's by Huffman coding.
//!
//! The blocks and pixels are those of OpenEXRCore on an x86-64 machine with AVX and F16C (any
//! from about 2011 on), whatever machine this runs on: its arithmetic there is in `simd.rs`,
//! and its lookup tables, which it makes with the C library's `powf` and `logf`, are those
//! glibc makes (`to_linear.bin`, `to_nonlinear.bin`). OpenEXRCore's own blocks differ on
//! other machines. The ZIP of the blocks is libdeflate's ([`crate::libdeflate`]), and the
//! Huffman coding that of PIZ as OpenEXRCore has it (`huf.rs`).
//!
//! Every channel is taken to be sampled at every pixel.

mod huf;
mod lossy;
mod simd;

use crate::libdeflate;

/// An `exr_result_t` of OpenEXRCore.
pub type Code = i32;
pub const OUT_OF_MEMORY: Code = 1;
pub const INVALID_ARGUMENT: Code = 3;
pub const ARGUMENT_OUT_OF_RANGE: Code = 4;
pub const BAD_CHUNK_LEADER: Code = 22;
pub const CORRUPT_CHUNK: Code = 23;

const PIXEL_UINT: i32 = 0;
const PIXEL_HALF: i32 = 1;
const PIXEL_FLOAT: i32 = 2;

static TO_LINEAR_BYTES: &[u8; 131072] = include_bytes!("to_linear.bin");
static TO_NONLINEAR_BYTES: &[u8; 131072] = include_bytes!("to_nonlinear.bin");

/// `exrcore_dwaToLinearTable` (or `ToNonLinear`): a half from the table of its bits.
fn table(bytes: &[u8; 131072], h: u16) -> u16 {
    let at = 2 * h as usize;
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn to_linear(h: u16) -> u16 {
    table(TO_LINEAR_BYTES, h)
}

fn to_nonlinear(h: u16) -> u16 {
    table(TO_NONLINEAR_BYTES, h)
}

/// A channel of the block.
#[derive(Clone, Copy, Debug)]
pub struct Channel<'a> {
    pub name: &'a str,
    /// `PIXELTYPE_*`: 0 unsigned integers, 1 halves, 2 floats.
    pub pixel_type: i32,
    /// Whether the channel says its values are linear (`pLinear`): they are then compressed
    /// as they are, and not by the lookup tables first.
    pub p_linear: bool,
}

impl Channel<'_> {
    fn bytes_per_element(&self) -> usize {
        if self.pixel_type == PIXEL_HALF {
            2
        } else {
            4
        }
    }
}

// ---------------------------------------------------------------------------------------
// The rules for a channel's compression (`internal_dwa_classifier.h`).

const UNKNOWN: u8 = 0;
const LOSSY_DCT: u8 = 1;
const RLE: u8 = 2;
const NUM_COMPRESSOR_SCHEMES: u8 = 3;

/// `Classifier`.
#[derive(Clone, Debug)]
struct Rule {
    suffix: Vec<u8>,
    scheme: u8,
    pixel_type: i32,
    csc_idx: i32,
    case_insensitive: bool,
}

/// The rules of `sDefaultChannelRules` or `sLegacyChannelRules`.
fn rules(legacy: bool) -> Vec<Rule> {
    let rule = |suffix: &str, scheme: u8, pixel_type: i32, csc_idx: i32| Rule {
        suffix: suffix.as_bytes().to_vec(),
        scheme,
        pixel_type,
        csc_idx,
        case_insensitive: legacy,
    };
    let mut out = Vec::new();
    let mut lossy = |suffix: &str, csc_idx: i32| {
        out.push(rule(suffix, LOSSY_DCT, PIXEL_HALF, csc_idx));
        out.push(rule(suffix, LOSSY_DCT, PIXEL_FLOAT, csc_idx));
    };
    if legacy {
        for (suffix, csc_idx) in [
            ("r", 0),
            ("red", 0),
            ("g", 1),
            ("grn", 1),
            ("green", 1),
            ("b", 2),
            ("blu", 2),
            ("blue", 2),
            ("y", -1),
            ("by", -1),
            ("ry", -1),
        ] {
            lossy(suffix, csc_idx);
        }
    } else {
        for (suffix, csc_idx) in [
            ("R", 0),
            ("G", 1),
            ("B", 2),
            ("Y", -1),
            ("BY", -1),
            ("RY", -1),
        ] {
            lossy(suffix, csc_idx);
        }
    }
    let alpha = if legacy { "a" } else { "A" };
    for pixel_type in [PIXEL_UINT, PIXEL_HALF, PIXEL_FLOAT] {
        out.push(rule(alpha, RLE, pixel_type, -1));
    }
    out
}

/// `Classifier_find_suffix`: what follows the last `.` of the name, or all of it.
fn find_suffix(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&byte| byte == b'.') {
        Some(at) => &name[at + 1..],
        None => name,
    }
}

/// `Classifier_match`.
fn rule_matches(rule: &Rule, suffix: &[u8], pixel_type: i32) -> bool {
    if rule.pixel_type != pixel_type {
        return false;
    }
    if rule.case_insensitive {
        return suffix.eq_ignore_ascii_case(&rule.suffix);
    }
    suffix == rule.suffix.as_slice()
}

/// `Classifier_size`.
fn rule_size(rule: &Rule) -> usize {
    rule.suffix.len() + 1 + 2
}

/// `Classifier_write`.
fn write_rule(rule: &Rule, out: &mut Vec<u8>) {
    out.extend_from_slice(&rule.suffix);
    out.push(0);
    let mut value = (((rule.csc_idx + 1) as u8) & 15) << 4;
    value |= (rule.scheme & 3) << 2;
    value |= u8::from(rule.case_insensitive);
    out.push(value);
    out.push(rule.pixel_type as u8);
}

/// `Classifier_read`: a rule from the start of `data`, and how many bytes it took.
fn read_rule(data: &[u8]) -> Result<(Rule, usize), Code> {
    let size = data.len();
    if size <= 3 {
        return Err(CORRUPT_CHUNK);
    }
    // (The suffix is no more than 128 bytes and its nil.)
    let mut len = 0usize;
    while len < 128 + 1 {
        if len > size - 3 {
            return Err(CORRUPT_CHUNK);
        }
        if data[len] == 0 {
            break;
        }
        len += 1;
    }
    if len == 128 + 1 {
        return Err(CORRUPT_CHUNK);
    }
    let suffix = data[..len].to_vec();
    len += 1;
    if size < len + 2 {
        return Err(CORRUPT_CHUNK);
    }
    let value = data[len];
    let pixel_type = data[len + 1];

    let csc_idx = i32::from(value >> 4) - 1;
    if !(-1..3).contains(&csc_idx) {
        return Err(CORRUPT_CHUNK);
    }
    let scheme = (value >> 2) & 3;
    if scheme >= NUM_COMPRESSOR_SCHEMES {
        return Err(CORRUPT_CHUNK);
    }
    let case_insensitive = value & 1 != 0;
    if pixel_type >= 3 {
        return Err(CORRUPT_CHUNK);
    }
    Ok((
        Rule {
            suffix,
            scheme,
            pixel_type: i32::from(pixel_type),
            csc_idx,
            case_insensitive,
        },
        len + 2,
    ))
}

// ---------------------------------------------------------------------------------------
// The compressor (`internal_dwa_compressor.h`).

/// What `DwaCompressor_classifyChannels` and `DwaCompressor_initializeBuffers` find of the
/// channels and the buffers they need.
struct Layout {
    /// Each channel's compression.
    compression: Vec<u8>,
    /// The channels converted to Y'CbCr together, red, green, blue.
    csc_sets: Vec<[usize; 3]>,
    out_buffer_size: usize,
    packed_ac_size: usize,
    packed_dc_size: usize,
    rle_buffer_size: usize,
    planar_size: [usize; 3],
    /// Where each channel's data starts in its scheme's planar buffer.
    planar_at: Vec<usize>,
}

fn classify(channels: &[Channel], rules: &[Rule]) -> (Vec<u8>, Vec<[usize; 3]>) {
    let mut compression = vec![UNKNOWN; channels.len()];
    // (prefix, the channels of each of its colours)
    let mut prefix_map: Vec<(&[u8], [i32; 3])> = Vec::new();
    for (c, channel) in channels.iter().enumerate() {
        let name = channel.name.as_bytes();
        let suffix = find_suffix(name);
        let prefix = &name[..name.len() - suffix.len()];
        let at = match prefix_map.iter().position(|(p, _)| *p == prefix) {
            Some(at) => at,
            None => {
                prefix_map.push((prefix, [-1; 3]));
                prefix_map.len() - 1
            }
        };
        for rule in rules {
            if rule_matches(rule, suffix, channel.pixel_type) {
                compression[c] = rule.scheme;
                if rule.csc_idx >= 0 {
                    prefix_map[at].1[rule.csc_idx as usize] = c as i32;
                }
            }
        }
    }
    let csc_sets = prefix_map
        .iter()
        .filter(|(_, idx)| idx.iter().all(|&i| i >= 0))
        .map(|(_, idx)| idx.map(|i| i as usize))
        .collect();
    (compression, csc_sets)
}

/// `ceilf((float) n / 8.0f)`.
fn blocks_of(n: usize) -> usize {
    ((n as f32) / 8.0).ceil() as usize
}

fn layout(channels: &[Channel], rules: &[Rule], width: usize, lines: usize) -> Layout {
    let (compression, csc_sets) = classify(channels, rules);
    let max_ac = blocks_of(lines) * blocks_of(width) * 63 * 2;
    let max_dc = blocks_of(lines) * blocks_of(width) * 2;
    let pixel_count = lines * width;

    let mut out_buffer_size: usize = rules.iter().map(rule_size).sum();
    let mut num_lossy = 0usize;
    let mut rle_buffer_size = 0usize;
    let mut unknown_buffer_size = 0usize;
    let mut planar_size = [0usize; 3];
    for (c, channel) in channels.iter().enumerate() {
        let bpe = channel.bytes_per_element();
        match compression[c] {
            LOSSY_DCT => {
                out_buffer_size += (2 * max_ac + 65536).max(libdeflate::compress_bound(max_ac));
                num_lossy += 1;
            }
            RLE => {
                rle_buffer_size += 2 * pixel_count * bpe;
                planar_size[RLE as usize] += 2 * pixel_count * bpe;
            }
            _ => {
                unknown_buffer_size += pixel_count * bpe;
                planar_size[UNKNOWN as usize] += pixel_count * bpe;
            }
        }
    }
    out_buffer_size += libdeflate::compress_bound(rle_buffer_size);
    out_buffer_size += libdeflate::compress_bound(unknown_buffer_size);
    out_buffer_size += libdeflate::compress_bound(max_dc * num_lossy);
    out_buffer_size += NUM_SIZES * 8;
    if planar_size[UNKNOWN as usize] > 0 {
        planar_size[UNKNOWN as usize] = libdeflate::compress_bound(planar_size[UNKNOWN as usize]);
    }

    // DwaCompressor_setupChannelData
    let mut next = [0usize; 3];
    let mut planar_at = Vec::with_capacity(channels.len());
    for (c, channel) in channels.iter().enumerate() {
        let scheme = compression[c] as usize;
        planar_at.push(next[scheme]);
        if compression[c] != LOSSY_DCT {
            next[scheme] += width * lines * channel.bytes_per_element();
        }
    }

    Layout {
        compression,
        csc_sets,
        out_buffer_size,
        packed_ac_size: max_ac * num_lossy,
        packed_dc_size: max_dc * num_lossy,
        rle_buffer_size,
        planar_size,
        planar_at,
    }
}

// The counters at the start of a block.
const VERSION: usize = 0;
const UNKNOWN_UNCOMPRESSED_SIZE: usize = 1;
const UNKNOWN_COMPRESSED_SIZE: usize = 2;
const AC_COMPRESSED_SIZE: usize = 3;
const DC_COMPRESSED_SIZE: usize = 4;
const RLE_COMPRESSED_SIZE: usize = 5;
const RLE_UNCOMPRESSED_SIZE: usize = 6;
const RLE_RAW_SIZE: usize = 7;
const AC_UNCOMPRESSED_COUNT: usize = 8;
const DC_UNCOMPRESSED_COUNT: usize = 9;
const AC_COMPRESSION: usize = 10;
const NUM_SIZES: usize = 11;

/// How the AC coefficients are compressed.
const STATIC_HUFFMAN: u64 = 0;
const DEFLATE: u64 = 1;

/// The zip level OpenEXRCore has for a part unless told otherwise (`-1`, libdeflate's 4).
const ZIP_LEVEL: i32 = -1;

// ---------------------------------------------------------------------------------------
// RLE (`internal_rle.c`) and the byte order of ZIP (`internal_zip.c`).

const MIN_RUN_LENGTH: usize = 3;
const MAX_RUN_LENGTH: usize = 127;

/// `internal_rle_compress`, into no more than about `out_bytes`.
fn rle_compress(src: &[u8], out_bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let end = src.len();
    let mut runs = 0usize;
    let mut rune = 1usize;
    while runs < end {
        let mut curcount = 0usize;
        while rune < end && src[runs] == src[rune] && curcount < MAX_RUN_LENGTH {
            rune += 1;
            curcount += 1;
        }
        if curcount >= MIN_RUN_LENGTH - 1 {
            out.push(curcount as u8);
            out.push(src[runs]);
            runs = rune;
        } else {
            // incompressible
            curcount += 1;
            while rune < end
                && ((rune + 1 >= end || src[rune] != src[rune + 1])
                    || (rune + 2 >= end || src[rune + 1] != src[rune + 2]))
                && curcount < MAX_RUN_LENGTH
            {
                curcount += 1;
                rune += 1;
            }
            out.push((-(curcount as i32)) as u8);
            out.extend_from_slice(&src[runs..rune]);
            runs = rune;
        }
        rune += 1;
        if out.len() >= out_bytes {
            break;
        }
    }
    out
}

/// `internal_rle_decompress`: the bytes made, or 0 for a stream that does not fit.
fn rle_decompress(out: &mut [u8], src: &[u8]) -> usize {
    let packsz = src.len();
    let outsz = out.len();
    let mut at = 0usize;
    let mut unpackbytes = 0usize;
    let mut outbytes = 0usize;
    while unpackbytes < packsz {
        let byte = src[at] as i8;
        if byte < 0 {
            let count = (-(i32::from(byte))) as usize;
            at += 1;
            unpackbytes += 1;
            if unpackbytes + count > packsz {
                return 0;
            }
            if outbytes + count > outsz {
                return 0;
            }
            out[outbytes..outbytes + count].copy_from_slice(&src[at..at + count]);
            at += count;
            unpackbytes += count;
            outbytes += count;
        } else {
            let count = byte as usize;
            at += 1;
            if unpackbytes + 2 > packsz {
                return 0;
            }
            unpackbytes += 2;
            let count = count + 1;
            if outbytes + count > outsz {
                return 0;
            }
            let value = src[at];
            out[outbytes..outbytes + count].fill(value);
            outbytes += count;
            at += 1;
        }
    }
    outbytes
}

/// `internal_zip_deconstruct_bytes`: the even bytes then the odd, as differences.
fn zip_deconstruct_bytes(source: &[u8]) -> Vec<u8> {
    let count = source.len();
    let mut scratch = vec![0u8; count];
    let half = count.div_ceil(2);
    for (i, &byte) in source.iter().enumerate() {
        if i % 2 == 0 {
            scratch[i / 2] = byte;
        } else {
            scratch[half + i / 2] = byte;
        }
    }
    let mut p = i32::from(*scratch.first().unwrap_or(&0));
    for t in scratch.iter_mut().skip(1) {
        let d = i32::from(*t) - p + (128 + 256);
        p = i32::from(*t);
        *t = d as u8;
    }
    scratch
}

/// `internal_zip_reconstruct_bytes`.
fn zip_reconstruct_bytes(source: &mut [u8]) -> Vec<u8> {
    for i in 1..source.len() {
        source[i] = (i32::from(source[i - 1]) + i32::from(source[i]) - 128) as u8;
    }
    let count = source.len();
    let half = count.div_ceil(2);
    (0..count)
        .map(|i| {
            if i % 2 == 0 {
                source[i / 2]
            } else {
                source[half + i / 2]
            }
        })
        .collect()
}

/// `exr_uncompress_buffer`: `input` into the start of `out`, the bytes made, or
/// OpenEXRCore's code for why not.
fn uncompress_buffer(input: &[u8], out: &mut [u8]) -> Result<usize, Code> {
    libdeflate::uncompress_buffer(input, out).map_err(|error| match error {
        libdeflate::Error::Corrupt => CORRUPT_CHUNK,
        libdeflate::Error::OutOfSpace => OUT_OF_MEMORY,
    })
}

// ---------------------------------------------------------------------------------------

/// `DwaCompressor_compress`: the block `raw` (its lines one after the other, each with its
/// channels one after the other) of `width` pixels and `lines` lines compressed at
/// `level` (`dwaCompressionLevel`), its AC coefficients by Huffman coding or (`deflate_ac`,
/// as OpenEXRCore has it for tiles of DWAA) by ZIP. A block that does not come out smaller
/// is given as it is.
pub fn compress(
    raw: &[u8],
    channels: &[Channel],
    width: usize,
    lines: usize,
    level: f32,
    deflate_ac: bool,
) -> Vec<u8> {
    let rules = rules(false);
    let layout = layout(channels, &rules, width, lines);
    let mut sizes = [0u64; NUM_SIZES];

    // The rules relevant to the channels, after the counters.
    let mut out = vec![0u8; NUM_SIZES * 8];
    let rules_at = out.len();
    out.extend_from_slice(&[0, 0]);
    for rule in &rules {
        if channels.iter().any(|channel| {
            rule_matches(
                rule,
                find_suffix(channel.name.as_bytes()),
                channel.pixel_type,
            )
        }) {
            write_rule(rule, &mut out);
        }
    }
    let rule_size = (out.len() - rules_at) as u16;
    out[rules_at..rules_at + 2].copy_from_slice(&rule_size.to_le_bytes());

    sizes[VERSION] = 2;
    sizes[AC_COMPRESSION] = if deflate_ac { DEFLATE } else { STATIC_HUFFMAN };

    // Where each channel's lines start in `raw`.
    let line_size: usize = channels
        .iter()
        .map(|channel| width * channel.bytes_per_element())
        .sum();
    let mut channel_at = Vec::with_capacity(channels.len());
    let mut at = 0usize;
    for channel in channels {
        channel_at.push(at);
        at += width * channel.bytes_per_element();
    }
    let rows =
        |c: usize| -> Vec<usize> { (0..lines).map(|y| y * line_size + channel_at[c]).collect() };

    let quant_base_error = level / 100000.0;
    let mut packed_ac: Vec<u16> = Vec::new();
    let mut packed_dc: Vec<u16> = Vec::new();
    let mut processed = vec![false; channels.len()];

    for set in &layout.csc_sets {
        let sources: Vec<lossy::Source> = set
            .iter()
            .map(|&c| lossy::Source {
                rows: rows(c),
                pixel_type: channels[c].pixel_type,
            })
            .collect();
        lossy::encode(
            raw,
            &sources,
            quant_base_error,
            true,
            width,
            lines,
            &mut packed_ac,
            &mut packed_dc,
        );
        for &c in set {
            processed[c] = true;
        }
    }

    let mut planar_unknown = vec![0u8; layout.planar_size[UNKNOWN as usize]];
    let mut planar_rle = vec![0u8; layout.planar_size[RLE as usize]];
    for (c, channel) in channels.iter().enumerate() {
        if processed[c] {
            continue;
        }
        let bpe = channel.bytes_per_element();
        let row_bytes = width * bpe;
        match layout.compression[c] {
            LOSSY_DCT => {
                let source = lossy::Source {
                    rows: rows(c),
                    pixel_type: channel.pixel_type,
                };
                lossy::encode(
                    raw,
                    &[source],
                    quant_base_error,
                    !channel.p_linear,
                    width,
                    lines,
                    &mut packed_ac,
                    &mut packed_dc,
                );
            }
            RLE => {
                // The first bytes of each value together, then the second, and so on.
                let plane = width * lines;
                let base = layout.planar_at[c];
                let mut end = [0usize; 4];
                for row in rows(c) {
                    for x in 0..width {
                        for (byte, e) in end.iter_mut().enumerate().take(bpe) {
                            planar_rle[base + byte * plane + *e] = raw[row + x * bpe + byte];
                            *e += 1;
                        }
                    }
                    sizes[RLE_RAW_SIZE] += row_bytes as u64;
                }
            }
            _ => {
                let base = layout.planar_at[c];
                for (y, row) in rows(c).into_iter().enumerate() {
                    planar_unknown[base + y * row_bytes..base + (y + 1) * row_bytes]
                        .copy_from_slice(&raw[row..row + row_bytes]);
                }
                sizes[UNKNOWN_UNCOMPRESSED_SIZE] += (width * lines * bpe) as u64;
            }
        }
        processed[c] = true;
    }
    sizes[AC_UNCOMPRESSED_COUNT] = packed_ac.len() as u64;
    sizes[DC_UNCOMPRESSED_COUNT] = packed_dc.len() as u64;

    if sizes[UNKNOWN_UNCOMPRESSED_SIZE] > 0 {
        let packed = libdeflate::compress_buffer(
            9,
            &planar_unknown[..sizes[UNKNOWN_UNCOMPRESSED_SIZE] as usize],
        );
        sizes[UNKNOWN_COMPRESSED_SIZE] = packed.len() as u64;
        out.extend_from_slice(&packed);
    }

    if !packed_ac.is_empty() {
        if deflate_ac {
            let bytes: Vec<u8> = packed_ac.iter().flat_map(|v| v.to_le_bytes()).collect();
            let packed = libdeflate::compress_buffer(9, &bytes);
            sizes[AC_COMPRESSED_SIZE] = packed.len() as u64;
            out.extend_from_slice(&packed);
        } else {
            match huf::compress(&packed_ac, layout.out_buffer_size - out.len()) {
                Ok(packed) => {
                    sizes[AC_COMPRESSED_SIZE] = packed.len() as u64;
                    out.extend_from_slice(&packed);
                }
                // (Stored as it is, the block too large for the coder.)
                Err(_) => return raw.to_vec(),
            }
        }
    }

    if !packed_dc.is_empty() {
        let bytes: Vec<u8> = packed_dc.iter().flat_map(|v| v.to_le_bytes()).collect();
        let packed = libdeflate::compress_buffer(ZIP_LEVEL, &zip_deconstruct_bytes(&bytes));
        sizes[DC_COMPRESSED_SIZE] = packed.len() as u64;
        out.extend_from_slice(&packed);
    }

    if sizes[RLE_RAW_SIZE] > 0 {
        let rle = rle_compress(
            &planar_rle[..sizes[RLE_RAW_SIZE] as usize],
            layout.rle_buffer_size,
        );
        sizes[RLE_UNCOMPRESSED_SIZE] = rle.len() as u64;
        let packed = libdeflate::compress_buffer(9, &rle);
        sizes[RLE_COMPRESSED_SIZE] = packed.len() as u64;
        out.extend_from_slice(&packed);
    }

    for (i, size) in sizes.iter().enumerate() {
        out[8 * i..8 * i + 8].copy_from_slice(&size.to_le_bytes());
    }
    if out.len() >= raw.len() {
        return raw.to_vec();
    }
    out
}

/// `DwaCompressor_uncompress`: the block `data` into `out`, which is as long as its lines,
/// or OpenEXRCore's code for why not.
pub fn uncompress(
    data: &[u8],
    channels: &[Channel],
    width: usize,
    lines: usize,
    out: &mut [u8],
) -> Result<(), Code> {
    let header_size = (NUM_SIZES * 8) as u64;
    let i_size = data.len() as u64;
    if i_size < header_size {
        return Err(CORRUPT_CHUNK);
    }
    out.fill(0);
    let uncompressed_size = out.len() as u64;

    let counter =
        |i: usize| u64::from_le_bytes(data[8 * i..8 * i + 8].try_into().expect("eight bytes"));
    let version = counter(VERSION);
    let unknown_uncompressed_size = counter(UNKNOWN_UNCOMPRESSED_SIZE);
    let unknown_compressed_size = counter(UNKNOWN_COMPRESSED_SIZE);
    let ac_compressed_size = counter(AC_COMPRESSED_SIZE);
    let dc_compressed_size = counter(DC_COMPRESSED_SIZE);
    let rle_compressed_size = counter(RLE_COMPRESSED_SIZE);
    let rle_uncompressed_size = counter(RLE_UNCOMPRESSED_SIZE);
    let rle_raw_size = counter(RLE_RAW_SIZE);
    let mut total_ac_count = counter(AC_UNCOMPRESSED_COUNT);
    let mut total_dc_count = counter(DC_UNCOMPRESSED_COUNT);
    let ac_compression = counter(AC_COMPRESSION);

    let compressed_size = unknown_compressed_size
        .wrapping_add(ac_compressed_size)
        .wrapping_add(dc_compressed_size)
        .wrapping_add(rle_compressed_size);

    // Both the sum and individual sizes are checked in case of overflow.
    if i_size < header_size.wrapping_add(compressed_size)
        || i_size < unknown_compressed_size
        || i_size < ac_compressed_size
        || i_size < dc_compressed_size
        || i_size < rle_compressed_size
    {
        return Err(CORRUPT_CHUNK);
    }
    if unknown_uncompressed_size > uncompressed_size
        || rle_raw_size > uncompressed_size
        || unknown_uncompressed_size.wrapping_add(rle_raw_size) > uncompressed_size
    {
        return Err(CORRUPT_CHUNK);
    }
    if [
        unknown_uncompressed_size,
        unknown_compressed_size,
        ac_compressed_size,
        dc_compressed_size,
        rle_compressed_size,
        rle_uncompressed_size,
        rle_raw_size,
        total_ac_count,
        total_dc_count,
    ]
    .iter()
    .any(|&size| (size as i64) < 0)
    {
        return Err(CORRUPT_CHUNK);
    }

    let mut data_at = header_size as usize;
    let rules = if version < 2 {
        rules(true)
    } else {
        // DwaCompressor_readChannelRules
        let n_avail = data.len() - data_at;
        if n_avail <= 2 {
            return Err(CORRUPT_CHUNK);
        }
        let rule_size = usize::from(u16::from_le_bytes([data[data_at], data[data_at + 1]]));
        if rule_size < 2 {
            return Err(CORRUPT_CHUNK);
        }
        if n_avail < rule_size {
            return Err(CORRUPT_CHUNK);
        }
        let mut rule_data = &data[data_at + 2..data_at + rule_size];
        data_at += rule_size;
        let mut rules = Vec::new();
        while !rule_data.is_empty() {
            let (rule, taken) = read_rule(rule_data)?;
            rules.push(rule);
            rule_data = &rule_data[taken..];
        }
        rules
    };

    let layout = layout(channels, &rules, width, lines);

    // UNKNOWN data is packed first, followed by the Huffman-compressed AC, then the DC
    // values, and then the zlib compressed RLE data.
    let unknown_at = data_at as u64;
    let ac_at = unknown_at + unknown_compressed_size;
    let dc_at = ac_at + ac_compressed_size;
    let rle_at = dc_at + dc_compressed_size;
    if ac_at > i_size || dc_at > i_size || rle_at > i_size || rle_at + rle_compressed_size > i_size
    {
        return Err(CORRUPT_CHUNK);
    }

    if version > 2 {
        return Err(BAD_CHUNK_LEADER);
    }

    // validate_size
    let validate =
        |scheme: u8, compressed: u64, uncompressed: u64, extra: u64| -> Result<(), Code> {
            let required: u64 = channels
                .iter()
                .enumerate()
                .filter(|(c, _)| layout.compression[*c] == scheme)
                .map(|(_, channel)| (width * lines * channel.bytes_per_element()) as u64)
                .sum();
            if required > 0 {
                if uncompressed < required || compressed == 0 {
                    return Err(CORRUPT_CHUNK);
                }
            } else if uncompressed > 0 || compressed > 0 || extra > 0 {
                return Err(CORRUPT_CHUNK);
            }
            Ok(())
        };
    validate(
        UNKNOWN,
        unknown_compressed_size,
        unknown_uncompressed_size,
        0,
    )?;
    validate(
        RLE,
        rle_compressed_size,
        rle_raw_size,
        rle_uncompressed_size,
    )?;

    let part = |at: u64, size: u64| &data[at as usize..(at + size) as usize];

    let mut planar_unknown = vec![0u8; layout.planar_size[UNKNOWN as usize]];
    if unknown_compressed_size > 0 {
        if unknown_uncompressed_size > planar_unknown.len() as u64 {
            return Err(CORRUPT_CHUNK);
        }
        let made = uncompress_buffer(
            part(unknown_at, unknown_compressed_size),
            &mut planar_unknown[..unknown_uncompressed_size as usize],
        )
        .map_err(|_| CORRUPT_CHUNK)?;
        if made as u64 != unknown_uncompressed_size {
            return Err(CORRUPT_CHUNK);
        }
    }

    let mut packed_ac = vec![0u16; layout.packed_ac_size / 2];
    if ac_compressed_size > 0 {
        if layout.packed_ac_size == 0 || total_ac_count * 2 > layout.packed_ac_size as u64 {
            return Err(CORRUPT_CHUNK);
        }
        let count = total_ac_count as usize;
        match ac_compression {
            STATIC_HUFFMAN => {
                huf::decompress(part(ac_at, ac_compressed_size), &mut packed_ac[..count])?
            }
            DEFLATE => {
                let mut bytes = vec![0u8; count * 2];
                let made = uncompress_buffer(part(ac_at, ac_compressed_size), &mut bytes)?;
                if made != count * 2 {
                    return Err(CORRUPT_CHUNK);
                }
                for (value, pair) in packed_ac.iter_mut().zip(bytes.chunks_exact(2)) {
                    *value = u16::from_le_bytes([pair[0], pair[1]]);
                }
            }
            _ => return Err(CORRUPT_CHUNK),
        }
    }

    let mut packed_dc = vec![0u16; layout.packed_dc_size / 2];
    if dc_compressed_size > 0 {
        let uncomp_bytes = total_dc_count * 2;
        if uncomp_bytes > layout.packed_dc_size as u64 {
            return Err(CORRUPT_CHUNK);
        }
        let mut scratch = vec![0u8; uncomp_bytes as usize];
        match uncompress_buffer(part(dc_at, dc_compressed_size), &mut scratch) {
            Ok(made) if made as u64 == uncomp_bytes => {}
            _ => return Err(CORRUPT_CHUNK),
        }
        let bytes = zip_reconstruct_bytes(&mut scratch);
        for (value, pair) in packed_dc.iter_mut().zip(bytes.chunks_exact(2)) {
            *value = u16::from_le_bytes([pair[0], pair[1]]);
        }
    } else if total_dc_count != 0 {
        // if the compressed size is 0, then the uncompressed size must also be zero
        return Err(CORRUPT_CHUNK);
    }

    let mut planar_rle = vec![0u8; layout.planar_size[RLE as usize]];
    if rle_raw_size > 0 {
        if rle_uncompressed_size > layout.rle_buffer_size as u64
            || rle_raw_size > planar_rle.len() as u64
        {
            return Err(CORRUPT_CHUNK);
        }
        let mut rle = vec![0u8; rle_uncompressed_size as usize];
        match uncompress_buffer(part(rle_at, rle_compressed_size), &mut rle) {
            Ok(made) if made as u64 == rle_uncompressed_size => {}
            _ => return Err(CORRUPT_CHUNK),
        }
        if rle_decompress(&mut planar_rle[..rle_raw_size as usize], &rle) as u64 != rle_raw_size {
            return Err(CORRUPT_CHUNK);
        }
    }

    // Where each channel's lines start in `out`.
    let line_size: usize = channels
        .iter()
        .map(|channel| width * channel.bytes_per_element())
        .sum();
    let mut channel_at = Vec::with_capacity(channels.len());
    let mut at = 0usize;
    for channel in channels {
        channel_at.push(at);
        at += width * channel.bytes_per_element();
    }
    let rows =
        |c: usize| -> Vec<usize> { (0..lines).map(|y| y * line_size + channel_at[c]).collect() };

    let mut decoder = lossy::Decoder::new(channels.len());
    let mut ac_at = 0usize;
    let mut dc_at = 0usize;
    let mut processed = vec![false; channels.len()];

    for set in &layout.csc_sets {
        if set.iter().any(|&c| layout.compression[c] != LOSSY_DCT) {
            return Err(CORRUPT_CHUNK);
        }
        let targets: Vec<lossy::Target> = set
            .iter()
            .map(|&c| lossy::Target {
                channel: c,
                rows: rows(c),
                pixel_type: channels[c].pixel_type,
            })
            .collect();
        let result = decoder.decode(
            out,
            &targets,
            &packed_ac,
            ac_at,
            total_ac_count,
            &packed_dc,
            dc_at,
            total_dc_count,
            true,
            width,
            lines,
        );
        ac_at += decoder.ac_count;
        dc_at += decoder.dc_count;
        total_ac_count -= decoder.ac_count as u64;
        total_dc_count -= decoder.dc_count as u64;
        for &c in set {
            processed[c] = true;
        }
        result?;
    }

    for (c, channel) in channels.iter().enumerate() {
        if processed[c] {
            continue;
        }
        if width == 0 || lines == 0 {
            processed[c] = true;
            continue;
        }
        let bpe = channel.bytes_per_element();
        match layout.compression[c] {
            LOSSY_DCT => {
                let target = lossy::Target {
                    channel: c,
                    rows: rows(c),
                    pixel_type: channel.pixel_type,
                };
                let result = decoder.decode(
                    out,
                    &[target],
                    &packed_ac,
                    ac_at,
                    total_ac_count,
                    &packed_dc,
                    dc_at,
                    total_dc_count,
                    !channel.p_linear,
                    width,
                    lines,
                );
                ac_at += decoder.ac_count;
                dc_at += decoder.dc_count;
                total_ac_count -= decoder.ac_count as u64;
                total_dc_count -= decoder.dc_count as u64;
                result?;
            }
            RLE => {
                let plane = width * lines;
                let base = layout.planar_at[c];
                let mut end = [0usize; 4];
                for row in rows(c) {
                    for x in 0..width {
                        for (byte, e) in end.iter_mut().enumerate().take(bpe) {
                            out[row + x * bpe + byte] = planar_rle[base + byte * plane + *e];
                            *e += 1;
                        }
                    }
                }
            }
            UNKNOWN => {
                let row_bytes = width * bpe;
                let mut at = layout.planar_at[c];
                for row in rows(c) {
                    // sanity check for buffer data lying within range
                    if at + row_bytes > planar_unknown.len() {
                        return Err(CORRUPT_CHUNK);
                    }
                    out[row..row + row_bytes].copy_from_slice(&planar_unknown[at..at + row_bytes]);
                    at += row_bytes;
                }
            }
            _ => return Err(CORRUPT_CHUNK),
        }
        processed[c] = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(
        width: usize,
        lines: usize,
        channels: &[Channel],
        value: impl Fn(usize, usize, usize) -> u32,
    ) -> Vec<u8> {
        let mut raw = Vec::new();
        for y in 0..lines {
            for (c, channel) in channels.iter().enumerate() {
                for x in 0..width {
                    let v = value(c, x, y);
                    if channel.pixel_type == PIXEL_HALF {
                        raw.extend_from_slice(&(v as u16).to_le_bytes());
                    } else {
                        raw.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        raw
    }

    #[test]
    fn what_is_kept_whole_comes_back_whole() {
        let channels = [
            Channel {
                name: "A",
                pixel_type: PIXEL_HALF,
                p_linear: false,
            },
            Channel {
                name: "id",
                pixel_type: PIXEL_UINT,
                p_linear: false,
            },
        ];
        let raw = block(37, 32, &channels, |c, x, y| {
            if c == 0 {
                0x3c00
            } else {
                (x / 5 + y / 3) as u32
            }
        });
        let packed = compress(&raw, &channels, 37, 32, 45.0, false);
        assert!(packed.len() < raw.len() / 4);
        let mut out = vec![0u8; raw.len()];
        uncompress(&packed, &channels, 37, 32, &mut out).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn colour_comes_back_near() {
        let channels: Vec<Channel> = ["B", "G", "R"]
            .iter()
            .map(|name| Channel {
                name,
                pixel_type: PIXEL_HALF,
                p_linear: false,
            })
            .collect();
        let half = |f: f32| simd::float_to_half(f) as u32;
        let raw = block(64, 40, &channels, |c, x, y| {
            half(0.25 + (x + 2 * y + 9 * c) as f32 / 200.0)
        });
        for deflate_ac in [false, true] {
            let packed = compress(&raw, &channels, 64, 40, 45.0, deflate_ac);
            assert!(packed.len() < raw.len() / 3);
            let mut out = vec![0u8; raw.len()];
            uncompress(&packed, &channels, 64, 40, &mut out).unwrap();
            for (a, b) in raw.chunks_exact(2).zip(out.chunks_exact(2)) {
                let (a, b) = (
                    simd::half_to_float(u16::from_le_bytes([a[0], a[1]])),
                    simd::half_to_float(u16::from_le_bytes([b[0], b[1]])),
                );
                assert!((a - b).abs() < 0.01, "{a} {b}");
            }
        }
    }

    #[test]
    fn damage_is_refused() {
        let channels = [Channel {
            name: "Y",
            pixel_type: PIXEL_FLOAT,
            p_linear: false,
        }];
        let raw = block(16, 16, &channels, |_, x, y| {
            (0.5 + (x * y) as f32 / 512.0).to_bits()
        });
        let mut packed = compress(&raw, &channels, 16, 16, 45.0, false);
        let mut out = vec![0u8; raw.len()];
        assert_eq!(
            uncompress(&packed[..40], &channels, 16, 16, &mut out),
            Err(CORRUPT_CHUNK)
        );
        packed[0] = 3;
        assert_eq!(
            uncompress(&packed, &channels, 16, 16, &mut out),
            Err(BAD_CHUNK_LEADER)
        );
    }
}
