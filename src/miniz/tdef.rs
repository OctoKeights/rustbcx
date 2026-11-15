//! The compressor: the port of `miniz_tdef.c`, as miniz builds it by default (without
//! `MINIZ_USE_UNALIGNED_LOADS_AND_STORES`, which is off on every processor unless asked for).
//!
//! [`tdefl_compress_mem_to_heap`] compresses a block in one go; a [`TdeflCompressor`] takes
//! its input a piece at a time ([`TdeflCompressor::compress_buffer`]) and hands the compressed
//! bytes over as they are made. What comes out does not depend on how the input was cut up.
//!
//! Left out: the output straight into a caller's buffer (`tdefl_compress` with an output
//! buffer; here the compressor always writes through its own buffer, which gives the same
//! bytes), `TDEFL_NONDETERMINISTIC_PARSING_FLAG` (which lets the output depend on what was in
//! memory before), and the PNG writer at the end of the file.

// (The arithmetic is written as the original has it.)
#![allow(clippy::manual_div_ceil)]

use super::mz_adler32;
use super::tables::{S_TDEFL_LARGE_DIST_EXTRA, S_TDEFL_LARGE_DIST_SYM, S_TDEFL_LEN_EXTRA, S_TDEFL_LEN_SYM, S_TDEFL_SMALL_DIST_EXTRA, S_TDEFL_SMALL_DIST_SYM};

// tdefl_init() compression flags logically OR'd together (low 12 bits contain the max. number of probes per dictionary search):
/// The compressor does no dictionary searches: Huffman coding only.
pub const TDEFL_HUFFMAN_ONLY: u32 = 0;
/// The compressor defaults to 128 dictionary probes per dictionary search. 0=Huffman only,
/// 1=Huffman+LZ (fastest/crap compression), 4095=Huffman+LZ (slowest/best compression).
pub const TDEFL_DEFAULT_MAX_PROBES: u32 = 128;
pub const TDEFL_MAX_PROBES_MASK: u32 = 0xFFF;

/// If set, the compressor outputs a zlib header before the deflate data, and the Adler-32 of
/// the source data at the end. Otherwise, you'll get raw deflate data.
pub const TDEFL_WRITE_ZLIB_HEADER: u32 = 0x01000;
/// Always compute the adler-32 of the input data (even when not writing zlib headers).
pub const TDEFL_COMPUTE_ADLER32: u32 = 0x02000;
/// Set to use faster greedy parsing, instead of more efficient lazy parsing.
pub const TDEFL_GREEDY_PARSING_FLAG: u32 = 0x04000;
/// Only look for RLE matches (matches with a distance of 1).
pub const TDEFL_RLE_MATCHES: u32 = 0x10000;
/// Discards matches <= 5 chars if enabled.
pub const TDEFL_FILTER_MATCHES: u32 = 0x20000;
/// Disable usage of optimized Huffman tables.
pub const TDEFL_FORCE_ALL_STATIC_BLOCKS: u32 = 0x40000;
/// Only use raw (uncompressed) deflate blocks.
pub const TDEFL_FORCE_ALL_RAW_BLOCKS: u32 = 0x80000;

const TDEFL_MAX_HUFF_TABLES: usize = 3;
const TDEFL_MAX_HUFF_SYMBOLS_0: usize = 288;
const TDEFL_MAX_HUFF_SYMBOLS_1: usize = 32;
const TDEFL_MAX_HUFF_SYMBOLS_2: usize = 19;
const TDEFL_LZ_DICT_SIZE: usize = 32768;
const TDEFL_LZ_DICT_SIZE_MASK: u32 = TDEFL_LZ_DICT_SIZE as u32 - 1;
const TDEFL_MIN_MATCH_LEN: u32 = 3;
const TDEFL_MAX_MATCH_LEN: u32 = 258;

// TDEFL_OUT_BUF_SIZE MUST be large enough to hold a single entire compressed output block (using static/fixed Huffman codes).
const TDEFL_LZ_CODE_BUF_SIZE: usize = 64 * 1024;
const TDEFL_OUT_BUF_SIZE: usize = (TDEFL_LZ_CODE_BUF_SIZE * 13) / 10;
const TDEFL_MAX_HUFF_SYMBOLS: usize = 288;
const TDEFL_LZ_HASH_BITS: u32 = 15;
const TDEFL_LZ_HASH_SHIFT: u32 = (TDEFL_LZ_HASH_BITS + 2) / 3;
const TDEFL_LZ_HASH_SIZE: usize = 1 << TDEFL_LZ_HASH_BITS;

/// The strategies of [`tdefl_create_comp_flags_from_zip_params`], as zlib numbers them.
pub const MZ_DEFAULT_STRATEGY: i32 = 0;
pub const MZ_FILTERED: i32 = 1;
pub const MZ_HUFFMAN_ONLY: i32 = 2;
pub const MZ_RLE: i32 = 3;
pub const MZ_FIXED: i32 = 4;
/// The level the others are measured against: 6.
pub const MZ_DEFAULT_LEVEL: i32 = 6;

/// What the compressor says after a call: `tdefl_status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TdeflStatus {
    BadParam = -2,
    PutBufFailed = -1,
    Okay = 0,
    Done = 1,
}

/// What to do with the input given so far: `tdefl_flush`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TdeflFlush {
    /// Hold on for more input.
    NoFlush = 0,
    /// Compress all that was given and end on a byte, so that all of it can be decompressed.
    SyncFlush = 2,
    /// The same, and forget what came before, so that decompression can start from here.
    FullFlush = 3,
    /// This was the last of the input.
    Finish = 4,
}

/// tdefl's compression state structure: `tdefl_compressor`.
pub struct TdeflCompressor {
    /// Where the compressed bytes go (the original's `m_pPut_buf_func`).
    out: Vec<u8>,
    m_flags: u32,
    m_max_probes: [u32; 2],
    m_greedy_parsing: bool,
    m_adler32: u32,
    m_lookahead_pos: u32,
    m_lookahead_size: u32,
    m_dict_size: u32,
    /// Places in `m_lz_code_buf`.
    m_p_lz_code_buf: usize,
    m_p_lz_flags: usize,
    /// Places in `m_output_buf`.
    m_p_output_buf: usize,
    m_p_output_buf_end: usize,
    m_num_flags_left: u32,
    m_total_lz_bytes: u32,
    m_lz_code_buf_dict_pos: u32,
    m_bits_in: u32,
    m_bit_buffer: u32,
    m_saved_match_dist: u32,
    m_saved_match_len: u32,
    m_saved_lit: u32,
    m_finished: bool,
    m_block_index: u32,
    m_wants_to_finish: bool,
    m_prev_return_status: TdeflStatus,
    m_flush: TdeflFlush,
    m_dict: Vec<u8>,
    m_huff_count: [[u16; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
    m_huff_codes: [[u16; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
    m_huff_code_sizes: [[u8; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
    m_lz_code_buf: Vec<u8>,
    m_next: Vec<u16>,
    m_hash: Vec<u16>,
    m_output_buf: Vec<u8>,
}

/// `tdefl_sym_freq`
#[derive(Clone, Copy, Default)]
struct TdeflSymFreq {
    m_key: u16,
    m_sym_index: u16,
}

/// Radix sorts tdefl_sym_freq[] array by 16-bit key m_key. Returns the sorted values (in one
/// of the two arrays; true for the second).
fn tdefl_radix_sort_syms(num_syms: usize, syms0: &mut [TdeflSymFreq], syms1: &mut [TdeflSymFreq]) -> bool {
    let mut total_passes = 2usize;
    let mut hist = [0u32; 256 * 2];
    for sym in &syms0[..num_syms] {
        let freq = usize::from(sym.m_key);
        hist[freq & 0xFF] += 1;
        hist[256 + ((freq >> 8) & 0xFF)] += 1;
    }
    while total_passes > 1 && num_syms as u32 == hist[(total_passes - 1) * 256] {
        total_passes -= 1;
    }
    let mut in_second = false;
    let mut pass_shift = 0;
    for pass in 0..total_passes {
        let p_hist = &hist[pass << 8..(pass << 8) + 256];
        let mut offsets = [0usize; 256];
        let mut cur_ofs = 0;
        for i in 0..256 {
            offsets[i] = cur_ofs;
            cur_ofs += p_hist[i] as usize;
        }
        let (cur, new): (&[TdeflSymFreq], &mut [TdeflSymFreq]) = if in_second { (&*syms1, &mut *syms0) } else { (&*syms0, &mut *syms1) };
        for sym in &cur[..num_syms] {
            let slot = &mut offsets[usize::from(sym.m_key >> pass_shift) & 0xFF];
            new[*slot] = *sym;
            *slot += 1;
        }
        in_second = !in_second;
        pass_shift += 8;
    }
    in_second
}

/// tdefl_calculate_minimum_redundancy() originally written by: Alistair Moffat, alistair@cs.mu.oz.au, Jyrki Katajainen, jyrki@diku.dk, November 1996.
fn tdefl_calculate_minimum_redundancy(a: &mut [TdeflSymFreq], n: usize) {
    if n == 0 {
        return;
    } else if n == 1 {
        a[0].m_key = 1;
        return;
    }
    let n = n as i32;
    a[0].m_key = a[0].m_key.wrapping_add(a[1].m_key);
    let mut root = 0i32;
    let mut leaf = 2i32;
    let mut next = 1i32;
    while next < n - 1 {
        if leaf >= n || a[root as usize].m_key < a[leaf as usize].m_key {
            a[next as usize].m_key = a[root as usize].m_key;
            a[root as usize].m_key = next as u16;
            root += 1;
        } else {
            a[next as usize].m_key = a[leaf as usize].m_key;
            leaf += 1;
        }
        if leaf >= n || (root < next && a[root as usize].m_key < a[leaf as usize].m_key) {
            a[next as usize].m_key = a[next as usize].m_key.wrapping_add(a[root as usize].m_key);
            a[root as usize].m_key = next as u16;
            root += 1;
        } else {
            a[next as usize].m_key = a[next as usize].m_key.wrapping_add(a[leaf as usize].m_key);
            leaf += 1;
        }
        next += 1;
    }
    a[(n - 2) as usize].m_key = 0;
    next = n - 3;
    while next >= 0 {
        a[next as usize].m_key = a[usize::from(a[next as usize].m_key)].m_key.wrapping_add(1);
        next -= 1;
    }
    let mut avbl = 1i32;
    let mut used = 0i32;
    let mut dpth = 0i32;
    root = n - 2;
    next = n - 1;
    while avbl > 0 {
        while root >= 0 && i32::from(a[root as usize].m_key) == dpth {
            used += 1;
            root -= 1;
        }
        while avbl > used {
            a[next as usize].m_key = dpth as u16;
            next -= 1;
            avbl -= 1;
        }
        avbl = 2 * used;
        dpth += 1;
        used = 0;
    }
}

// Limits canonical Huffman code table's max code size.
const TDEFL_MAX_SUPPORTED_HUFF_CODESIZE: usize = 32;

fn tdefl_huffman_enforce_max_code_size(num_codes: &mut [i32; 1 + TDEFL_MAX_SUPPORTED_HUFF_CODESIZE], code_list_len: usize, max_code_size: usize) {
    let mut total = 0u32;
    if code_list_len <= 1 {
        return;
    }
    for i in max_code_size + 1..=TDEFL_MAX_SUPPORTED_HUFF_CODESIZE {
        num_codes[max_code_size] += num_codes[i];
    }
    for i in (1..=max_code_size).rev() {
        total = total.wrapping_add((num_codes[i] as u32) << (max_code_size - i));
    }
    while total != (1u32 << max_code_size) {
        num_codes[max_code_size] -= 1;
        for i in (1..max_code_size).rev() {
            if num_codes[i] != 0 {
                num_codes[i] -= 1;
                num_codes[i + 1] += 2;
                break;
            }
        }
        total = total.wrapping_sub(1);
    }
}

const S_TDEFL_PACKED_CODE_SIZE_SYMS_SWIZZLE: [u8; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

const MZ_BITMASKS: [u32; 17] = [0x0000, 0x0001, 0x0003, 0x0007, 0x000F, 0x001F, 0x003F, 0x007F, 0x00FF, 0x01FF, 0x03FF, 0x07FF, 0x0FFF, 0x1FFF, 0x3FFF, 0x7FFF, 0xFFFF];

const S_TDEFL_NUM_PROBES: [u32; 11] = [0, 1, 6, 32, 16, 32, 128, 256, 512, 768, 1500];

impl TdeflCompressor {
    /// Initializes the compressor: `tdefl_init`, with the bytes it makes kept until
    /// [`TdeflCompressor::take_output`] takes them.
    ///
    /// `flags`: the max match finder probes (default is 128) logically OR'd against the
    /// `TDEFL_*` flags. Higher probes are slower but improve compression.
    pub fn new(flags: u32) -> TdeflCompressor {
        TdeflCompressor {
            out: Vec::new(),
            m_flags: flags,
            m_max_probes: [1 + ((flags & 0xFFF) + 2) / 3, 1 + (((flags & 0xFFF) >> 2) + 2) / 3],
            m_greedy_parsing: flags & TDEFL_GREEDY_PARSING_FLAG != 0,
            m_adler32: 1,
            m_lookahead_pos: 0,
            m_lookahead_size: 0,
            m_dict_size: 0,
            m_p_lz_code_buf: 1,
            m_p_lz_flags: 0,
            m_p_output_buf: 0,
            m_p_output_buf_end: 0,
            m_num_flags_left: 8,
            m_total_lz_bytes: 0,
            m_lz_code_buf_dict_pos: 0,
            m_bits_in: 0,
            m_bit_buffer: 0,
            m_saved_match_dist: 0,
            m_saved_match_len: 0,
            m_saved_lit: 0,
            m_finished: false,
            m_block_index: 0,
            m_wants_to_finish: false,
            m_prev_return_status: TdeflStatus::Okay,
            m_flush: TdeflFlush::NoFlush,
            m_dict: vec![0; TDEFL_LZ_DICT_SIZE + TDEFL_MAX_MATCH_LEN as usize - 1],
            m_huff_count: [[0; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
            m_huff_codes: [[0; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
            m_huff_code_sizes: [[0; TDEFL_MAX_HUFF_SYMBOLS]; TDEFL_MAX_HUFF_TABLES],
            m_lz_code_buf: vec![0; TDEFL_LZ_CODE_BUF_SIZE],
            m_next: vec![0; TDEFL_LZ_DICT_SIZE],
            m_hash: vec![0; TDEFL_LZ_HASH_SIZE],
            m_output_buf: vec![0; TDEFL_OUT_BUF_SIZE],
        }
    }

    /// The compressed bytes made so far and not yet taken.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// `tdefl_get_prev_return_status`
    pub fn prev_return_status(&self) -> TdeflStatus {
        self.m_prev_return_status
    }

    /// The Adler-32 of the input so far, when the flags ask for it: `tdefl_get_adler32`.
    pub fn adler32(&self) -> u32 {
        self.m_adler32
    }

    fn tdefl_optimize_huffman_table(&mut self, table_num: usize, table_len: usize, code_size_limit: usize, static_table: bool) {
        let mut num_codes = [0i32; 1 + TDEFL_MAX_SUPPORTED_HUFF_CODESIZE];
        let mut next_code = [0u32; TDEFL_MAX_SUPPORTED_HUFF_CODESIZE + 1];
        if static_table {
            for i in 0..table_len {
                num_codes[usize::from(self.m_huff_code_sizes[table_num][i])] += 1;
            }
        } else {
            let mut syms0 = [TdeflSymFreq::default(); TDEFL_MAX_HUFF_SYMBOLS];
            let mut syms1 = [TdeflSymFreq::default(); TDEFL_MAX_HUFF_SYMBOLS];
            let mut num_used_syms = 0;
            for i in 0..table_len {
                let count = self.m_huff_count[table_num][i];
                if count != 0 {
                    syms0[num_used_syms].m_key = count;
                    syms0[num_used_syms].m_sym_index = i as u16;
                    num_used_syms += 1;
                }
            }

            let in_second = tdefl_radix_sort_syms(num_used_syms, &mut syms0, &mut syms1);
            let syms = if in_second { &mut syms1 } else { &mut syms0 };
            tdefl_calculate_minimum_redundancy(syms, num_used_syms);

            for sym in &syms[..num_used_syms] {
                num_codes[usize::from(sym.m_key)] += 1;
            }

            tdefl_huffman_enforce_max_code_size(&mut num_codes, num_used_syms, code_size_limit);

            self.m_huff_code_sizes[table_num] = [0; TDEFL_MAX_HUFF_SYMBOLS];
            self.m_huff_codes[table_num] = [0; TDEFL_MAX_HUFF_SYMBOLS];
            let mut j = num_used_syms;
            for i in 1..=code_size_limit {
                for _ in 0..num_codes[i] {
                    j -= 1;
                    self.m_huff_code_sizes[table_num][usize::from(syms[j].m_sym_index)] = i as u8;
                }
            }
        }

        next_code[1] = 0;
        let mut j = 0u32;
        for i in 2..=code_size_limit {
            j = (j.wrapping_add(num_codes[i - 1] as u32)) << 1;
            next_code[i] = j;
        }

        for i in 0..table_len {
            let mut rev_code = 0u32;
            let code_size = usize::from(self.m_huff_code_sizes[table_num][i]);
            if code_size == 0 {
                continue;
            }
            let mut code = next_code[code_size];
            next_code[code_size] += 1;
            for _ in 0..code_size {
                rev_code = (rev_code << 1) | (code & 1);
                code >>= 1;
            }
            self.m_huff_codes[table_num][i] = rev_code as u16;
        }
    }

    /// `TDEFL_PUT_BITS`
    #[inline]
    fn put_bits(&mut self, bits: u32, len: u32) {
        debug_assert!(bits <= ((1u32 << len) - 1));
        self.m_bit_buffer |= bits << self.m_bits_in;
        self.m_bits_in += len;
        while self.m_bits_in >= 8 {
            if self.m_p_output_buf < self.m_p_output_buf_end {
                self.m_output_buf[self.m_p_output_buf] = self.m_bit_buffer as u8;
                self.m_p_output_buf += 1;
            }
            self.m_bit_buffer >>= 8;
            self.m_bits_in -= 8;
        }
    }

    fn tdefl_start_dynamic_block(&mut self) {
        let mut code_sizes_to_pack = [0u8; TDEFL_MAX_HUFF_SYMBOLS_0 + TDEFL_MAX_HUFF_SYMBOLS_1];
        let mut packed_code_sizes = [0u8; TDEFL_MAX_HUFF_SYMBOLS_0 + TDEFL_MAX_HUFF_SYMBOLS_1];
        let mut prev_code_size = 0xFFu8;

        self.m_huff_count[0][256] = 1;

        self.tdefl_optimize_huffman_table(0, TDEFL_MAX_HUFF_SYMBOLS_0, 15, false);
        self.tdefl_optimize_huffman_table(1, TDEFL_MAX_HUFF_SYMBOLS_1, 15, false);

        let mut num_lit_codes = 286;
        while num_lit_codes > 257 {
            if self.m_huff_code_sizes[0][num_lit_codes - 1] != 0 {
                break;
            }
            num_lit_codes -= 1;
        }
        let mut num_dist_codes = 30;
        while num_dist_codes > 1 {
            if self.m_huff_code_sizes[1][num_dist_codes - 1] != 0 {
                break;
            }
            num_dist_codes -= 1;
        }

        code_sizes_to_pack[..num_lit_codes].copy_from_slice(&self.m_huff_code_sizes[0][..num_lit_codes]);
        code_sizes_to_pack[num_lit_codes..num_lit_codes + num_dist_codes].copy_from_slice(&self.m_huff_code_sizes[1][..num_dist_codes]);
        let total_code_sizes_to_pack = num_lit_codes + num_dist_codes;
        let mut num_packed_code_sizes = 0usize;
        let mut rle_z_count = 0u32;
        let mut rle_repeat_count = 0u32;

        self.m_huff_count[2][..TDEFL_MAX_HUFF_SYMBOLS_2].fill(0);

        // TDEFL_RLE_PREV_CODE_SIZE
        fn rle_prev_code_size(count: &mut [u16; TDEFL_MAX_HUFF_SYMBOLS], packed: &mut [u8], num_packed: &mut usize, rle_repeat_count: &mut u32, prev_code_size: u8) {
            if *rle_repeat_count != 0 {
                if *rle_repeat_count < 3 {
                    count[usize::from(prev_code_size)] = count[usize::from(prev_code_size)].wrapping_add(*rle_repeat_count as u16);
                    while *rle_repeat_count != 0 {
                        *rle_repeat_count -= 1;
                        packed[*num_packed] = prev_code_size;
                        *num_packed += 1;
                    }
                } else {
                    count[16] = count[16].wrapping_add(1);
                    packed[*num_packed] = 16;
                    packed[*num_packed + 1] = (*rle_repeat_count - 3) as u8;
                    *num_packed += 2;
                }
                *rle_repeat_count = 0;
            }
        }
        // TDEFL_RLE_ZERO_CODE_SIZE
        fn rle_zero_code_size(count: &mut [u16; TDEFL_MAX_HUFF_SYMBOLS], packed: &mut [u8], num_packed: &mut usize, rle_z_count: &mut u32) {
            if *rle_z_count != 0 {
                if *rle_z_count < 3 {
                    count[0] = count[0].wrapping_add(*rle_z_count as u16);
                    while *rle_z_count != 0 {
                        *rle_z_count -= 1;
                        packed[*num_packed] = 0;
                        *num_packed += 1;
                    }
                } else if *rle_z_count <= 10 {
                    count[17] = count[17].wrapping_add(1);
                    packed[*num_packed] = 17;
                    packed[*num_packed + 1] = (*rle_z_count - 3) as u8;
                    *num_packed += 2;
                } else {
                    count[18] = count[18].wrapping_add(1);
                    packed[*num_packed] = 18;
                    packed[*num_packed + 1] = (*rle_z_count - 11) as u8;
                    *num_packed += 2;
                }
                *rle_z_count = 0;
            }
        }

        for i in 0..total_code_sizes_to_pack {
            let code_size = code_sizes_to_pack[i];
            if code_size == 0 {
                rle_prev_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_repeat_count, prev_code_size);
                rle_z_count += 1;
                if rle_z_count == 138 {
                    rle_zero_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_z_count);
                }
            } else {
                rle_zero_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_z_count);
                if code_size != prev_code_size {
                    rle_prev_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_repeat_count, prev_code_size);
                    self.m_huff_count[2][usize::from(code_size)] = self.m_huff_count[2][usize::from(code_size)].wrapping_add(1);
                    packed_code_sizes[num_packed_code_sizes] = code_size;
                    num_packed_code_sizes += 1;
                } else {
                    rle_repeat_count += 1;
                    if rle_repeat_count == 6 {
                        rle_prev_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_repeat_count, prev_code_size);
                    }
                }
            }
            prev_code_size = code_size;
        }
        if rle_repeat_count != 0 {
            rle_prev_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_repeat_count, prev_code_size);
        } else {
            rle_zero_code_size(&mut self.m_huff_count[2], &mut packed_code_sizes, &mut num_packed_code_sizes, &mut rle_z_count);
        }

        self.tdefl_optimize_huffman_table(2, TDEFL_MAX_HUFF_SYMBOLS_2, 7, false);

        self.put_bits(2, 2);

        self.put_bits(num_lit_codes as u32 - 257, 5);
        self.put_bits(num_dist_codes as u32 - 1, 5);

        let mut num_bit_lengths = 18i32;
        while num_bit_lengths >= 0 {
            if self.m_huff_code_sizes[2][usize::from(S_TDEFL_PACKED_CODE_SIZE_SYMS_SWIZZLE[num_bit_lengths as usize])] != 0 {
                break;
            }
            num_bit_lengths -= 1;
        }
        let num_bit_lengths = 4.max(num_bit_lengths + 1) as usize;
        self.put_bits(num_bit_lengths as u32 - 4, 4);
        for i in 0..num_bit_lengths {
            self.put_bits(u32::from(self.m_huff_code_sizes[2][usize::from(S_TDEFL_PACKED_CODE_SIZE_SYMS_SWIZZLE[i])]), 3);
        }

        let mut packed_code_sizes_index = 0;
        while packed_code_sizes_index < num_packed_code_sizes {
            let code = usize::from(packed_code_sizes[packed_code_sizes_index]);
            packed_code_sizes_index += 1;
            debug_assert!(code < TDEFL_MAX_HUFF_SYMBOLS_2);
            self.put_bits(u32::from(self.m_huff_codes[2][code]), u32::from(self.m_huff_code_sizes[2][code]));
            if code >= 16 {
                self.put_bits(u32::from(packed_code_sizes[packed_code_sizes_index]), [2, 3, 7][code - 16]);
                packed_code_sizes_index += 1;
            }
        }
    }

    fn tdefl_start_static_block(&mut self) {
        let p = &mut self.m_huff_code_sizes[0];
        p[..=143].fill(8);
        p[144..=255].fill(9);
        p[256..=279].fill(7);
        p[280..=287].fill(8);

        self.m_huff_code_sizes[1][..32].fill(5);

        self.tdefl_optimize_huffman_table(0, 288, 15, true);
        self.tdefl_optimize_huffman_table(1, 32, 15, true);

        self.put_bits(1, 2);
    }

    fn tdefl_compress_lz_codes(&mut self) -> bool {
        let mut flags = 1u32;
        let mut p_lz_codes = 0usize;
        while p_lz_codes < self.m_p_lz_code_buf {
            if flags == 1 {
                flags = u32::from(self.m_lz_code_buf[p_lz_codes]) | 0x100;
                p_lz_codes += 1;
            }
            if flags & 1 != 0 {
                let match_len = usize::from(self.m_lz_code_buf[p_lz_codes]);
                let match_dist = usize::from(self.m_lz_code_buf[p_lz_codes + 1]) | (usize::from(self.m_lz_code_buf[p_lz_codes + 2]) << 8);
                p_lz_codes += 3;

                let len_sym = usize::from(S_TDEFL_LEN_SYM[match_len]);
                debug_assert!(self.m_huff_code_sizes[0][len_sym] != 0);
                self.put_bits(u32::from(self.m_huff_codes[0][len_sym]), u32::from(self.m_huff_code_sizes[0][len_sym]));
                self.put_bits(match_len as u32 & MZ_BITMASKS[usize::from(S_TDEFL_LEN_EXTRA[match_len])], u32::from(S_TDEFL_LEN_EXTRA[match_len]));

                let (sym, num_extra_bits) = if match_dist < 512 {
                    (usize::from(S_TDEFL_SMALL_DIST_SYM[match_dist]), u32::from(S_TDEFL_SMALL_DIST_EXTRA[match_dist]))
                } else {
                    (usize::from(S_TDEFL_LARGE_DIST_SYM[match_dist >> 8]), u32::from(S_TDEFL_LARGE_DIST_EXTRA[match_dist >> 8]))
                };
                debug_assert!(self.m_huff_code_sizes[1][sym] != 0);
                self.put_bits(u32::from(self.m_huff_codes[1][sym]), u32::from(self.m_huff_code_sizes[1][sym]));
                self.put_bits(match_dist as u32 & MZ_BITMASKS[num_extra_bits as usize], num_extra_bits);
            } else {
                let lit = usize::from(self.m_lz_code_buf[p_lz_codes]);
                p_lz_codes += 1;
                debug_assert!(self.m_huff_code_sizes[0][lit] != 0);
                self.put_bits(u32::from(self.m_huff_codes[0][lit]), u32::from(self.m_huff_code_sizes[0][lit]));
            }
            flags >>= 1;
        }

        self.put_bits(u32::from(self.m_huff_codes[0][256]), u32::from(self.m_huff_code_sizes[0][256]));

        self.m_p_output_buf < self.m_p_output_buf_end
    }

    fn tdefl_compress_block(&mut self, static_block: bool) -> bool {
        if static_block {
            self.tdefl_start_static_block();
        } else {
            self.tdefl_start_dynamic_block();
        }
        self.tdefl_compress_lz_codes()
    }

    /// `tdefl_flush_block`. (It gives back the bytes still to hand over, which here are none:
    /// every block goes out whole.)
    fn tdefl_flush_block(&mut self, flush: TdeflFlush) -> i32 {
        let mut comp_block_succeeded = false;
        let use_raw_block = (self.m_flags & TDEFL_FORCE_ALL_RAW_BLOCKS != 0) && self.m_lookahead_pos.wrapping_sub(self.m_lz_code_buf_dict_pos) <= self.m_dict_size;

        self.m_p_output_buf = 0;
        self.m_p_output_buf_end = TDEFL_OUT_BUF_SIZE - 16;

        // (All eight bits can be shifted out.)
        self.m_lz_code_buf[self.m_p_lz_flags] = (u32::from(self.m_lz_code_buf[self.m_p_lz_flags]) >> self.m_num_flags_left) as u8;
        self.m_p_lz_code_buf -= usize::from(self.m_num_flags_left == 8);

        if (self.m_flags & TDEFL_WRITE_ZLIB_HEADER != 0) && self.m_block_index == 0 {
            let cmf = 0x78u32;
            let mut flevel = 3u32;

            // Determine compression level by reversing the process in tdefl_create_comp_flags_from_zip_params()
            let mut i = 0;
            while i < S_TDEFL_NUM_PROBES.len() {
                if S_TDEFL_NUM_PROBES[i] == (self.m_flags & 0xFFF) {
                    break;
                }
                i += 1;
            }

            if i < 2 {
                flevel = 0;
            } else if i < 6 {
                flevel = 1;
            } else if i == 6 {
                flevel = 2;
            }

            let mut header = cmf << 8 | (flevel << 6);
            header += 31 - (header % 31);
            let flg = header & 0xFF;

            self.put_bits(cmf, 8);
            self.put_bits(flg, 8);
        }

        self.put_bits(u32::from(flush == TdeflFlush::Finish), 1);

        let p_saved_output_buf = self.m_p_output_buf;
        let saved_bit_buf = self.m_bit_buffer;
        let saved_bits_in = self.m_bits_in;

        if !use_raw_block {
            comp_block_succeeded = self.tdefl_compress_block((self.m_flags & TDEFL_FORCE_ALL_STATIC_BLOCKS != 0) || (self.m_total_lz_bytes < 48));
        }

        // If the block gets expanded, forget the current contents of the output buffer and send a raw block instead.
        if (use_raw_block || (self.m_total_lz_bytes != 0 && (self.m_p_output_buf - p_saved_output_buf + 1) as u32 >= self.m_total_lz_bytes)) && self.m_lookahead_pos.wrapping_sub(self.m_lz_code_buf_dict_pos) <= self.m_dict_size {
            self.m_p_output_buf = p_saved_output_buf;
            self.m_bit_buffer = saved_bit_buf;
            self.m_bits_in = saved_bits_in;
            self.put_bits(0, 2);
            if self.m_bits_in != 0 {
                self.put_bits(0, 8 - self.m_bits_in);
            }
            for _ in 0..2 {
                self.put_bits(self.m_total_lz_bytes & 0xFFFF, 16);
                self.m_total_lz_bytes ^= 0xFFFF;
            }
            for i in 0..self.m_total_lz_bytes {
                let byte = self.m_dict[(self.m_lz_code_buf_dict_pos.wrapping_add(i) & TDEFL_LZ_DICT_SIZE_MASK) as usize];
                self.put_bits(u32::from(byte), 8);
            }
        }
        // Check for the extremely unlikely (if not impossible) case of the compressed block not fitting into the output buffer when using dynamic codes.
        else if !comp_block_succeeded {
            self.m_p_output_buf = p_saved_output_buf;
            self.m_bit_buffer = saved_bit_buf;
            self.m_bits_in = saved_bits_in;
            self.tdefl_compress_block(true);
        }

        if flush != TdeflFlush::NoFlush {
            if flush == TdeflFlush::Finish {
                if self.m_bits_in != 0 {
                    self.put_bits(0, 8 - self.m_bits_in);
                }
                if self.m_flags & TDEFL_WRITE_ZLIB_HEADER != 0 {
                    let mut a = self.m_adler32;
                    for _ in 0..4 {
                        self.put_bits((a >> 24) & 0xFF, 8);
                        a <<= 8;
                    }
                }
            } else {
                let mut z = 0u32;
                self.put_bits(0, 3);
                if self.m_bits_in != 0 {
                    self.put_bits(0, 8 - self.m_bits_in);
                }
                for _ in 0..2 {
                    self.put_bits(z & 0xFFFF, 16);
                    z ^= 0xFFFF;
                }
            }
        }

        debug_assert!(self.m_p_output_buf < self.m_p_output_buf_end);

        self.m_huff_count[0][..TDEFL_MAX_HUFF_SYMBOLS_0].fill(0);
        self.m_huff_count[1][..TDEFL_MAX_HUFF_SYMBOLS_1].fill(0);

        self.m_p_lz_code_buf = 1;
        self.m_p_lz_flags = 0;
        self.m_num_flags_left = 8;
        self.m_lz_code_buf_dict_pos = self.m_lz_code_buf_dict_pos.wrapping_add(self.m_total_lz_bytes);
        self.m_total_lz_bytes = 0;
        self.m_block_index += 1;

        let n = self.m_p_output_buf;
        if n != 0 {
            self.out.extend_from_slice(&self.m_output_buf[..n]);
        }

        0
    }

    #[inline]
    fn tdefl_find_match(&self, lookahead_pos: u32, max_dist: u32, max_match_len: u32, p_match_dist: &mut u32, p_match_len: &mut u32) {
        let mut dist = 0u32;
        let pos = (lookahead_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize;
        let mut match_len = *p_match_len;
        let mut probe_pos = pos;
        let mut num_probes_left = self.m_max_probes[usize::from(match_len >= 32)];
        let dict = &self.m_dict;
        let mut c0 = dict[pos + match_len as usize];
        let mut c1 = dict[pos + match_len as usize - 1];
        debug_assert!(max_match_len <= TDEFL_MAX_MATCH_LEN);
        if max_match_len <= match_len {
            return;
        }
        loop {
            loop {
                num_probes_left -= 1;
                if num_probes_left == 0 {
                    return;
                }
                // TDEFL_PROBE, three times over.
                let mut found = false;
                for _ in 0..3 {
                    let next_probe_pos = u32::from(self.m_next[probe_pos]);
                    if next_probe_pos == 0 {
                        return;
                    }
                    dist = u32::from(lookahead_pos.wrapping_sub(next_probe_pos) as u16);
                    if dist > max_dist {
                        return;
                    }
                    probe_pos = (next_probe_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize;
                    if dict[probe_pos + match_len as usize] == c0 && dict[probe_pos + match_len as usize - 1] == c1 {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            if dist == 0 {
                break;
            }
            let mut probe_len = 0u32;
            while probe_len < max_match_len {
                if dict[pos + probe_len as usize] != dict[probe_pos + probe_len as usize] {
                    break;
                }
                probe_len += 1;
            }
            if probe_len > match_len {
                *p_match_dist = dist;
                match_len = probe_len;
                *p_match_len = match_len;
                if match_len == max_match_len {
                    return;
                }
                c0 = dict[pos + match_len as usize];
                c1 = dict[pos + match_len as usize - 1];
            }
        }
    }

    #[inline]
    fn tdefl_record_literal(&mut self, lit: u8) {
        self.m_total_lz_bytes += 1;
        self.m_lz_code_buf[self.m_p_lz_code_buf] = lit;
        self.m_p_lz_code_buf += 1;
        self.m_lz_code_buf[self.m_p_lz_flags] >>= 1;
        self.m_num_flags_left -= 1;
        if self.m_num_flags_left == 0 {
            self.m_num_flags_left = 8;
            self.m_p_lz_flags = self.m_p_lz_code_buf;
            self.m_p_lz_code_buf += 1;
        }
        self.m_huff_count[0][usize::from(lit)] = self.m_huff_count[0][usize::from(lit)].wrapping_add(1);
    }

    #[inline]
    fn tdefl_record_match(&mut self, match_len: u32, mut match_dist: u32) {
        debug_assert!(match_len >= TDEFL_MIN_MATCH_LEN && match_dist >= 1 && match_dist <= TDEFL_LZ_DICT_SIZE as u32);

        self.m_total_lz_bytes += match_len;

        self.m_lz_code_buf[self.m_p_lz_code_buf] = (match_len - TDEFL_MIN_MATCH_LEN) as u8;

        match_dist -= 1;
        self.m_lz_code_buf[self.m_p_lz_code_buf + 1] = (match_dist & 0xFF) as u8;
        self.m_lz_code_buf[self.m_p_lz_code_buf + 2] = (match_dist >> 8) as u8;
        self.m_p_lz_code_buf += 3;

        self.m_lz_code_buf[self.m_p_lz_flags] = (self.m_lz_code_buf[self.m_p_lz_flags] >> 1) | 0x80;
        self.m_num_flags_left -= 1;
        if self.m_num_flags_left == 0 {
            self.m_num_flags_left = 8;
            self.m_p_lz_flags = self.m_p_lz_code_buf;
            self.m_p_lz_code_buf += 1;
        }

        let s0 = S_TDEFL_SMALL_DIST_SYM[(match_dist & 511) as usize];
        let s1 = S_TDEFL_LARGE_DIST_SYM[((match_dist >> 8) & 127) as usize];
        let dist_sym = usize::from(if match_dist < 512 { s0 } else { s1 });
        self.m_huff_count[1][dist_sym] = self.m_huff_count[1][dist_sym].wrapping_add(1);
        let len_sym = usize::from(S_TDEFL_LEN_SYM[(match_len - TDEFL_MIN_MATCH_LEN) as usize]);
        self.m_huff_count[0][len_sym] = self.m_huff_count[0][len_sym].wrapping_add(1);
    }

    /// `tdefl_compress_normal`. Gives back how much of `src` it took.
    fn tdefl_compress_normal(&mut self, src: &[u8]) -> usize {
        let mut p_src = 0usize;
        let mut src_buf_left = src.len();
        let flush = self.m_flush;

        while src_buf_left != 0 || (flush != TdeflFlush::NoFlush && self.m_lookahead_size != 0) {
            // Update dictionary and hash chains. Keeps the lookahead size equal to TDEFL_MAX_MATCH_LEN.
            if self.m_lookahead_size + self.m_dict_size >= TDEFL_MIN_MATCH_LEN - 1 {
                let mut dst_pos = (self.m_lookahead_pos.wrapping_add(self.m_lookahead_size) & TDEFL_LZ_DICT_SIZE_MASK) as usize;
                let mut ins_pos = self.m_lookahead_pos.wrapping_add(self.m_lookahead_size).wrapping_sub(2);
                let mut hash = (u32::from(self.m_dict[(ins_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize]) << TDEFL_LZ_HASH_SHIFT) ^ u32::from(self.m_dict[(ins_pos.wrapping_add(1) & TDEFL_LZ_DICT_SIZE_MASK) as usize]);
                let num_bytes_to_process = src_buf_left.min((TDEFL_MAX_MATCH_LEN - self.m_lookahead_size) as usize);
                let p_src_end = p_src + num_bytes_to_process;
                src_buf_left -= num_bytes_to_process;
                self.m_lookahead_size += num_bytes_to_process as u32;
                while p_src != p_src_end {
                    let c = src[p_src];
                    p_src += 1;
                    self.m_dict[dst_pos] = c;
                    if dst_pos < (TDEFL_MAX_MATCH_LEN - 1) as usize {
                        self.m_dict[TDEFL_LZ_DICT_SIZE + dst_pos] = c;
                    }
                    hash = ((hash << TDEFL_LZ_HASH_SHIFT) ^ u32::from(c)) & (TDEFL_LZ_HASH_SIZE as u32 - 1);
                    self.m_next[(ins_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize] = self.m_hash[hash as usize];
                    self.m_hash[hash as usize] = ins_pos as u16;
                    dst_pos = (dst_pos + 1) & TDEFL_LZ_DICT_SIZE_MASK as usize;
                    ins_pos = ins_pos.wrapping_add(1);
                }
            } else {
                while src_buf_left != 0 && self.m_lookahead_size < TDEFL_MAX_MATCH_LEN {
                    let c = src[p_src];
                    p_src += 1;
                    let dst_pos = (self.m_lookahead_pos.wrapping_add(self.m_lookahead_size) & TDEFL_LZ_DICT_SIZE_MASK) as usize;
                    src_buf_left -= 1;
                    self.m_dict[dst_pos] = c;
                    if dst_pos < (TDEFL_MAX_MATCH_LEN - 1) as usize {
                        self.m_dict[TDEFL_LZ_DICT_SIZE + dst_pos] = c;
                    }
                    self.m_lookahead_size += 1;
                    if self.m_lookahead_size + self.m_dict_size >= TDEFL_MIN_MATCH_LEN {
                        let ins_pos = self.m_lookahead_pos.wrapping_add(self.m_lookahead_size - 1).wrapping_sub(2);
                        let hash = ((u32::from(self.m_dict[(ins_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize]) << (TDEFL_LZ_HASH_SHIFT * 2)) ^ (u32::from(self.m_dict[(ins_pos.wrapping_add(1) & TDEFL_LZ_DICT_SIZE_MASK) as usize]) << TDEFL_LZ_HASH_SHIFT) ^ u32::from(c)) & (TDEFL_LZ_HASH_SIZE as u32 - 1);
                        self.m_next[(ins_pos & TDEFL_LZ_DICT_SIZE_MASK) as usize] = self.m_hash[hash as usize];
                        self.m_hash[hash as usize] = ins_pos as u16;
                    }
                }
            }
            self.m_dict_size = (TDEFL_LZ_DICT_SIZE as u32 - self.m_lookahead_size).min(self.m_dict_size);
            if flush == TdeflFlush::NoFlush && self.m_lookahead_size < TDEFL_MAX_MATCH_LEN {
                break;
            }

            // Simple lazy/greedy parsing state machine.
            let mut len_to_move = 1u32;
            let mut cur_match_dist = 0u32;
            let mut cur_match_len = if self.m_saved_match_len != 0 { self.m_saved_match_len } else { TDEFL_MIN_MATCH_LEN - 1 };
            let cur_pos = self.m_lookahead_pos & TDEFL_LZ_DICT_SIZE_MASK;
            if self.m_flags & (TDEFL_RLE_MATCHES | TDEFL_FORCE_ALL_RAW_BLOCKS) != 0 {
                if self.m_dict_size != 0 && (self.m_flags & TDEFL_FORCE_ALL_RAW_BLOCKS == 0) {
                    let c = self.m_dict[(cur_pos.wrapping_sub(1) & TDEFL_LZ_DICT_SIZE_MASK) as usize];
                    cur_match_len = 0;
                    while cur_match_len < self.m_lookahead_size {
                        if self.m_dict[(cur_pos + cur_match_len) as usize] != c {
                            break;
                        }
                        cur_match_len += 1;
                    }
                    if cur_match_len < TDEFL_MIN_MATCH_LEN {
                        cur_match_len = 0;
                    } else {
                        cur_match_dist = 1;
                    }
                }
            } else {
                self.tdefl_find_match(self.m_lookahead_pos, self.m_dict_size, self.m_lookahead_size, &mut cur_match_dist, &mut cur_match_len);
            }
            if (cur_match_len == TDEFL_MIN_MATCH_LEN && cur_match_dist >= 8 * 1024) || cur_pos == cur_match_dist || ((self.m_flags & TDEFL_FILTER_MATCHES != 0) && cur_match_len <= 5) {
                cur_match_dist = 0;
                cur_match_len = 0;
            }
            let dict_last = self.m_dict.len() - 1;
            if self.m_saved_match_len != 0 {
                if cur_match_len > self.m_saved_match_len {
                    self.tdefl_record_literal(self.m_saved_lit as u8);
                    if cur_match_len >= 128 {
                        self.tdefl_record_match(cur_match_len, cur_match_dist);
                        self.m_saved_match_len = 0;
                        len_to_move = cur_match_len;
                    } else {
                        self.m_saved_lit = u32::from(self.m_dict[cur_pos as usize]);
                        self.m_saved_match_dist = cur_match_dist;
                        self.m_saved_match_len = cur_match_len;
                    }
                } else {
                    self.tdefl_record_match(self.m_saved_match_len, self.m_saved_match_dist);
                    len_to_move = self.m_saved_match_len - 1;
                    self.m_saved_match_len = 0;
                }
            } else if cur_match_dist == 0 {
                self.tdefl_record_literal(self.m_dict[(cur_pos as usize).min(dict_last)]);
            } else if self.m_greedy_parsing || (self.m_flags & TDEFL_RLE_MATCHES != 0) || cur_match_len >= 128 {
                self.tdefl_record_match(cur_match_len, cur_match_dist);
                len_to_move = cur_match_len;
            } else {
                self.m_saved_lit = u32::from(self.m_dict[(cur_pos as usize).min(dict_last)]);
                self.m_saved_match_dist = cur_match_dist;
                self.m_saved_match_len = cur_match_len;
            }
            // Move the lookahead forward by len_to_move bytes.
            self.m_lookahead_pos = self.m_lookahead_pos.wrapping_add(len_to_move);
            debug_assert!(self.m_lookahead_size >= len_to_move);
            self.m_lookahead_size -= len_to_move;
            self.m_dict_size = (self.m_dict_size + len_to_move).min(TDEFL_LZ_DICT_SIZE as u32);
            // Check if it's time to flush the current LZ codes to the internal output buffer.
            if self.m_p_lz_code_buf > TDEFL_LZ_CODE_BUF_SIZE - 8 || (self.m_total_lz_bytes > 31 * 1024 && ((((self.m_p_lz_code_buf as u32) * 115) >> 7) >= self.m_total_lz_bytes || (self.m_flags & TDEFL_FORCE_ALL_RAW_BLOCKS != 0))) {
                self.tdefl_flush_block(TdeflFlush::NoFlush);
            }
        }

        p_src
    }

    /// Compresses a piece of the input, all of it, and says what to do with what has been
    /// given so far: `tdefl_compress_buffer`. The compressed bytes are kept for
    /// [`TdeflCompressor::take_output`]. After [`TdeflFlush::Finish`] the answer is
    /// [`TdeflStatus::Done`], and the compressor takes no more.
    pub fn compress_buffer(&mut self, in_buf: &[u8], flush: TdeflFlush) -> TdeflStatus {
        self.m_flush = flush;

        if self.m_prev_return_status != TdeflStatus::Okay || (self.m_wants_to_finish && flush != TdeflFlush::Finish) {
            self.m_prev_return_status = TdeflStatus::BadParam;
            return self.m_prev_return_status;
        }
        self.m_wants_to_finish |= flush == TdeflFlush::Finish;

        if self.m_finished {
            self.m_prev_return_status = TdeflStatus::Done;
            return self.m_prev_return_status;
        }

        let taken = self.tdefl_compress_normal(in_buf);

        if self.m_flags & (TDEFL_WRITE_ZLIB_HEADER | TDEFL_COMPUTE_ADLER32) != 0 {
            self.m_adler32 = mz_adler32(self.m_adler32, &in_buf[..taken]);
        }

        if flush != TdeflFlush::NoFlush && self.m_lookahead_size == 0 && taken == in_buf.len() {
            self.tdefl_flush_block(flush);
            self.m_finished = flush == TdeflFlush::Finish;
            if flush == TdeflFlush::FullFlush {
                self.m_hash.fill(0);
                self.m_next.fill(0);
                self.m_dict_size = 0;
            }
        }

        self.m_prev_return_status = if self.m_finished { TdeflStatus::Done } else { TdeflStatus::Okay };
        self.m_prev_return_status
    }
}

/// Compresses a block in memory: `tdefl_compress_mem_to_heap`.
///
/// `flags`: the max match finder probes (default is 128) logically OR'd against the `TDEFL_*`
/// flags; [`tdefl_create_comp_flags_from_zip_params`] makes them from a level. What comes
/// back could be larger than the block on uncompressible data.
pub fn tdefl_compress_mem_to_heap(src_buf: &[u8], flags: u32) -> Vec<u8> {
    let mut comp = TdeflCompressor::new(flags);
    let status = comp.compress_buffer(src_buf, TdeflFlush::Finish);
    debug_assert!(status == TdeflStatus::Done);
    comp.take_output()
}

/// Create tdefl_compress() flags given zlib-style compression parameters.
///
/// `level` may range from 0 to 10 (where 10 is absolute max compression, but may be much slower
/// on some files). Below 0 it takes the probes of the default level, 6, with the greedy
/// parsing of levels 0 to 3: that is the original's arithmetic, kept. `window_bits` may be -15 (raw deflate) or 15
/// (zlib). `strategy` may be either `MZ_DEFAULT_STRATEGY`, `MZ_FILTERED`, `MZ_HUFFMAN_ONLY`,
/// `MZ_RLE`, or `MZ_FIXED`.
pub fn tdefl_create_comp_flags_from_zip_params(level: i32, window_bits: i32, strategy: i32) -> u32 {
    let mut comp_flags = S_TDEFL_NUM_PROBES[if level >= 0 { level.min(10) } else { MZ_DEFAULT_LEVEL } as usize] | if level <= 3 { TDEFL_GREEDY_PARSING_FLAG } else { 0 };
    if window_bits > 0 {
        comp_flags |= TDEFL_WRITE_ZLIB_HEADER;
    }

    if level == 0 {
        comp_flags |= TDEFL_FORCE_ALL_RAW_BLOCKS;
    } else if strategy == MZ_FILTERED {
        comp_flags |= TDEFL_FILTER_MATCHES;
    } else if strategy == MZ_HUFFMAN_ONLY {
        comp_flags &= !TDEFL_MAX_PROBES_MASK;
    } else if strategy == MZ_FIXED {
        comp_flags |= TDEFL_FORCE_ALL_STATIC_BLOCKS;
    } else if strategy == MZ_RLE {
        comp_flags |= TDEFL_RLE_MATCHES;
    }

    comp_flags
}
