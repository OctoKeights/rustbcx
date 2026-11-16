//! The decompressor: a port of `deflate_decompress.c` (with `decompress_template.h`),
//! `zlib_decompress.c` and `adler32.c` of libdeflate v1.26.
//!
//! The original decodes with tables, in a fast loop while far from the ends of its buffers and
//! in a careful one near them; both make the same output and stop for the same reasons. The
//! port decodes as the careful loop does, codeword by codeword, and refills its bits at the
//! same points, so that a damaged stream ends in the same way: what the original reads past
//! the end of its input is taken as zeros, up to eight bytes of them.

// (The arithmetic is written as the original has it.)
#![allow(clippy::manual_is_multiple_of)]

use super::Result as DeflateResult;

const BLOCKTYPE_UNCOMPRESSED: u64 = 0;
const BLOCKTYPE_STATIC_HUFFMAN: u64 = 1;
const BLOCKTYPE_DYNAMIC_HUFFMAN: u64 = 2;

const NUM_PRECODE_SYMS: usize = 19;
const MAX_PRE_CODEWORD_LEN: u32 = 7;
const MAX_LITLEN_CODEWORD_LEN: u32 = 15;
const MAX_OFFSET_CODEWORD_LEN: u32 = 15;

/// `CONSUMABLE_NBITS`: what a refill fills the bit buffer to, at least (63 - 7).
const CONSUMABLE_NBITS: u32 = 56;

/// The base of each length symbol, from 257, and its extra bits (`litlen_decode_results`;
/// the two symbols the format does not use are lengths of 258 too).
const LENGTH_BASE: [(u32, u32); 31] = [
    (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 0),
    (11, 1), (13, 1), (15, 1), (17, 1), (19, 2), (23, 2), (27, 2), (31, 2),
    (35, 3), (43, 3), (51, 3), (59, 3), (67, 4), (83, 4), (99, 4), (115, 4),
    (131, 5), (163, 5), (195, 5), (227, 5), (258, 0), (258, 0), (258, 0),
];

/// The base of each offset symbol and its extra bits (`offset_decode_results`; the two
/// symbols the format does not use are as the last it does).
const OFFSET_BASE: [(u32, u32); 32] = [
    (1, 0), (2, 0), (3, 0), (4, 0), (5, 1), (7, 1), (9, 2), (13, 2),
    (17, 3), (25, 3), (33, 4), (49, 4), (65, 5), (97, 5), (129, 6), (193, 6),
    (257, 7), (385, 7), (513, 8), (769, 8), (1025, 9), (1537, 9), (2049, 10), (3073, 10),
    (4097, 11), (6145, 11), (8193, 12), (12289, 12), (16385, 13), (24577, 13), (24577, 13), (24577, 13),
];

/// `deflate_precode_lens_permutation`.
const PRECODE_LENS_PERMUTATION: [usize; NUM_PRECODE_SYMS] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// A Huffman code to decode by: for each `max_len`-bit window of the stream (bit-reversed,
/// the first bit the lowest), the symbol and the length of its codeword.
struct Code {
    max_len: u32,
    table: Vec<(u16, u8)>,
}

impl Code {
    /// `build_decode_table`: the canonical code of `lens`, or none where they do not form
    /// one the original takes: one that overflows the code space, or leaves part of it
    /// unused but with nothing in it, or with one codeword of one bit (each of those, the
    /// original gives a symbol both codewords of one bit: the one there is, or the first).
    fn new(lens: &[u8], max_codeword_len: u32) -> Option<Code> {
        let mut len_counts = [0u32; 16];
        for &len in lens {
            len_counts[len as usize] += 1;
        }
        // The longest codeword there is.
        let mut max_len = max_codeword_len;
        while max_len > 1 && len_counts[max_len as usize] == 0 {
            max_len -= 1;
        }
        let mut codespace_used = 0u32;
        for count in &len_counts[1..=max_len as usize] {
            codespace_used = (codespace_used << 1) + count;
        }

        // overfull code?
        if codespace_used > (1 << max_len) {
            return None;
        }
        // incomplete code?
        if codespace_used < (1 << max_len) {
            let sym = if codespace_used == 0 {
                0 // arbitrary
            } else {
                if codespace_used != 1 << (max_len - 1) || len_counts[1] != 1 {
                    return None;
                }
                lens.iter().position(|&len| len != 0).expect("one codeword") as u16
            };
            return Some(Code { max_len: 1, table: vec![(sym, 1); 2] });
        }

        // The codewords in order: by length, then by symbol.
        let mut table = vec![(0u16, 0u8); 1 << max_len];
        let mut code = 0u32;
        for len in 1..=max_len {
            for (sym, &l) in lens.iter().enumerate() {
                if u32::from(l) != len {
                    continue;
                }
                // Every window that starts with the codeword, read the other way round.
                let reversed = code.reverse_bits() >> (32 - len);
                let mut window = reversed;
                while window < (1 << max_len) {
                    table[window as usize] = (sym as u16, len as u8);
                    window += 1 << len;
                }
                code += 1;
            }
            code <<= 1;
        }
        Some(Code { max_len, table })
    }
}

/// Why a stream did not decompress (`enum libdeflate_result`, but success).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    BadData,
    InsufficientSpace,
}

/// The bits of the input, as the original keeps them: refilled byte by byte to at least 56,
/// with zeros past the end, of which no more than 8 bytes may be taken.
struct Bits<'a> {
    input: &'a [u8],
    next: usize,
    bitbuf: u64,
    bitsleft: u32,
    overread_count: usize,
}

impl Bits<'_> {
    /// `REFILL_BITS`.
    fn refill(&mut self) -> Result<(), Stop> {
        while self.bitsleft < CONSUMABLE_NBITS {
            if self.next != self.input.len() {
                self.bitbuf |= u64::from(self.input[self.next]) << self.bitsleft;
                self.next += 1;
            } else {
                self.overread_count += 1;
                if self.overread_count > 8 {
                    return Err(Stop::BadData);
                }
            }
            self.bitsleft += 8;
        }
        Ok(())
    }

    fn take(&mut self, n: u32) -> u32 {
        let value = (self.bitbuf & ((1u64 << n) - 1)) as u32;
        self.bitbuf >>= n;
        self.bitsleft -= n;
        value
    }

    /// A symbol of `code` (which the buffer has bits enough for).
    fn decode(&mut self, code: &Code) -> u16 {
        let (sym, len) = code.table[(self.bitbuf & ((1u64 << code.max_len) - 1)) as usize];
        self.bitbuf >>= len;
        self.bitsleft -= u32::from(len);
        sym
    }
}

/// `libdeflate_deflate_decompress_ex`: the raw DEFLATE stream at the start of `input` into
/// `out`. The answer is the bytes taken of the input and the bytes made.
fn deflate_decompress(input: &[u8], out: &mut [u8]) -> Result<(usize, usize), Stop> {
    let mut bits = Bits { input, next: 0, bitbuf: 0, bitsleft: 0, overread_count: 0 };
    let mut out_next = 0usize;
    let out_end = out.len();

    loop {
        // next_block:
        bits.refill()?;
        let is_final_block = bits.bitbuf & 1 != 0;
        let block_type = (bits.bitbuf >> 1) & 3;

        let (litlen, offset);
        if block_type == BLOCKTYPE_DYNAMIC_HUFFMAN {
            let num_litlen_syms = 257 + ((bits.bitbuf >> 3) & 31) as usize;
            let num_offset_syms = 1 + ((bits.bitbuf >> 8) & 31) as usize;
            let num_explicit_precode_lens = 4 + ((bits.bitbuf >> 13) & 15) as usize;

            // (CAN_CONSUME(3 * 18): the first length with the header, then a refill.)
            let mut precode_lens = [0u8; NUM_PRECODE_SYMS];
            precode_lens[PRECODE_LENS_PERMUTATION[0]] = ((bits.bitbuf >> 17) & 7) as u8;
            bits.bitbuf >>= 20;
            bits.bitsleft -= 20;
            bits.refill()?;
            for &sym in &PRECODE_LENS_PERMUTATION[1..num_explicit_precode_lens] {
                precode_lens[sym] = bits.take(3) as u8;
            }
            let precode = Code::new(&precode_lens, MAX_PRE_CODEWORD_LEN).ok_or(Stop::BadData)?;

            // The codeword lengths of both codes, one after the other.
            let total = num_litlen_syms + num_offset_syms;
            let mut lens = vec![0u8; total + 137];
            let mut i = 0usize;
            while i < total {
                if bits.bitsleft < MAX_PRE_CODEWORD_LEN + 7 {
                    bits.refill()?;
                }
                let presym = bits.decode(&precode);
                if presym < 16 {
                    lens[i] = presym as u8;
                    i += 1;
                    continue;
                }
                if presym == 16 {
                    if i == 0 {
                        return Err(Stop::BadData);
                    }
                    let rep_val = lens[i - 1];
                    let rep_count = 3 + bits.take(2) as usize;
                    lens[i..i + rep_count].fill(rep_val);
                    i += rep_count;
                } else if presym == 17 {
                    let rep_count = 3 + bits.take(3) as usize;
                    lens[i..i + rep_count].fill(0);
                    i += rep_count;
                } else {
                    let rep_count = 11 + bits.take(7) as usize;
                    lens[i..i + rep_count].fill(0);
                    i += rep_count;
                }
            }
            if i != total {
                return Err(Stop::BadData);
            }
            // (The offset code first, as the original builds them.)
            offset = Code::new(&lens[num_litlen_syms..total], MAX_OFFSET_CODEWORD_LEN).ok_or(Stop::BadData)?;
            litlen = Code::new(&lens[..num_litlen_syms], MAX_LITLEN_CODEWORD_LEN).ok_or(Stop::BadData)?;
        } else if block_type == BLOCKTYPE_UNCOMPRESSED {
            bits.bitsleft -= 3; // for BTYPE and BFINAL
            let bytes_left = (bits.bitsleft >> 3) as usize;
            if bits.overread_count > bytes_left {
                return Err(Stop::BadData);
            }
            bits.next -= bytes_left - bits.overread_count;
            bits.overread_count = 0;
            bits.bitbuf = 0;
            bits.bitsleft = 0;

            if input.len() - bits.next < 4 {
                return Err(Stop::BadData);
            }
            let len = u16::from_le_bytes([input[bits.next], input[bits.next + 1]]) as usize;
            let nlen = u16::from_le_bytes([input[bits.next + 2], input[bits.next + 3]]);
            bits.next += 4;
            if len as u16 != !nlen {
                return Err(Stop::BadData);
            }
            if len > out_end - out_next {
                return Err(Stop::InsufficientSpace);
            }
            if len > input.len() - bits.next {
                return Err(Stop::BadData);
            }
            out[out_next..out_next + len].copy_from_slice(&input[bits.next..bits.next + len]);
            bits.next += len;
            out_next += len;
            if is_final_block {
                break;
            }
            continue;
        } else {
            if block_type != BLOCKTYPE_STATIC_HUFFMAN {
                return Err(Stop::BadData);
            }
            bits.bitbuf >>= 3; // for BTYPE and BFINAL
            bits.bitsleft -= 3;
            let mut lens = [0u8; 288 + 32];
            lens[..144].fill(8);
            lens[144..256].fill(9);
            lens[256..280].fill(7);
            lens[280..288].fill(8);
            lens[288..].fill(5);
            offset = Code::new(&lens[288..], MAX_OFFSET_CODEWORD_LEN).expect("the static code");
            litlen = Code::new(&lens[..288], MAX_LITLEN_CODEWORD_LEN).expect("the static code");
        }

        // generic_loop:
        loop {
            bits.refill()?;
            let sym = bits.decode(&litlen) as usize;
            if sym < 256 {
                if out_next == out_end {
                    return Err(Stop::InsufficientSpace);
                }
                out[out_next] = sym as u8;
                out_next += 1;
                continue;
            }
            if sym == 256 {
                break; // block_done
            }
            let (base, extra) = LENGTH_BASE[sym - 257];
            let length = (base + bits.take(extra)) as usize;
            if length > out_end - out_next {
                return Err(Stop::InsufficientSpace);
            }
            // (The buffer has bits enough for an offset without a refill.)
            let (base, extra) = OFFSET_BASE[bits.decode(&offset) as usize];
            let distance = (base + bits.take(extra)) as usize;
            if distance > out_next {
                return Err(Stop::BadData);
            }
            for k in 0..length {
                out[out_next + k] = out[out_next + k - distance];
            }
            out_next += length;
        }

        // block_done:
        if is_final_block {
            break;
        }
    }

    let bytes_left = (bits.bitsleft >> 3) as usize;
    if bits.overread_count > bytes_left {
        return Err(Stop::BadData);
    }
    Ok((bits.next - (bytes_left - bits.overread_count), out_next))
}

/// `libdeflate_adler32`.
pub(crate) fn adler32(adler: u32, data: &[u8]) -> u32 {
    const DIVISOR: u32 = 65521;
    let (mut s1, mut s2) = (adler & 0xffff, adler >> 16);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            s1 += u32::from(byte);
            s2 += s1;
        }
        s1 %= DIVISOR;
        s2 %= DIVISOR;
    }
    (s2 << 16) | s1
}

/// `libdeflate_zlib_decompress_ex`: the zlib stream `input` into `out`, as many bytes as it
/// makes (at most `out.len()`). The answer is the bytes taken of the input and the bytes made.
fn zlib_decompress(input: &[u8], out: &mut [u8]) -> Result<(usize, usize), Stop> {
    if input.len() < 2 + 4 {
        return Err(Stop::BadData);
    }

    // 2 byte header: CMF and FLG
    let hdr = u16::from_be_bytes([input[0], input[1]]);
    // FCHECK
    if hdr % 31 != 0 {
        return Err(Stop::BadData);
    }
    // CM
    if (hdr >> 8) & 0xF != 8 {
        return Err(Stop::BadData);
    }
    // CINFO
    if (hdr >> 12) > 7 {
        return Err(Stop::BadData);
    }
    // FDICT
    if (hdr >> 5) & 1 != 0 {
        return Err(Stop::BadData);
    }

    // Compressed data
    let (actual_in, actual_out) = deflate_decompress(&input[2..input.len() - 4], out)?;
    let at = 2 + actual_in;

    // ADLER32
    if adler32(1, &out[..actual_out]) != u32::from_be_bytes(input[at..at + 4].try_into().expect("four bytes")) {
        return Err(Stop::BadData);
    }
    Ok((at + 4, actual_out))
}

/// `exr_uncompress_buffer` of OpenEXR 3.5 (`compression.c`): the zlib stream `input` into
/// `out`, with libdeflate's answer as OpenEXR gives it: the bytes made, or why not (a stream
/// that does not take all of `input` is corrupt).
pub fn uncompress_buffer(input: &[u8], out: &mut [u8]) -> DeflateResult<usize> {
    match zlib_decompress(input, out) {
        Ok((actual_in, actual_out)) if actual_in == input.len() => Ok(actual_out),
        Ok(_) | Err(Stop::BadData) => Err(super::Error::Corrupt),
        Err(Stop::InsufficientSpace) => Err(super::Error::OutOfSpace),
    }
}
