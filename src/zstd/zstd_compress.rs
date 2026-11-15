//! Bytes to a Zstandard frame: a port of part of `lib/compress/zstd_compress.c` and
//! `lib/compress/zstd_compress_internal.h` of zstd v1.5.7, the compressor that the `zstd`
//! program and libzstd are.
//!
//! [`compress`] gives the bytes that `ZSTD_compress` gives for the same source and level,
//! into an output of `ZSTD_compressBound` bytes, as a 64-bit build has it. What is ported
//! is the path of that one call: one frame from a source that is all there, with the size
//! of the content in its header, no checksum and no dictionary.
//!
//! Of the nine strategies the original has for finding matches, the first two are ported:
//! `ZSTD_fast` and `ZSTD_dfast`. Which strategy a level takes depends on the size of the
//! source; they cover levels 1 to 3 and all the negative levels for every size, and level 4
//! for sources over 256 KB and from 16 KB to 128 KB. A level that takes another strategy is
//! refused: the port has no approximation of it.

// (The arithmetic is written as the original has it.)
#![allow(clippy::manual_div_ceil, clippy::int_plus_one, clippy::manual_clamp)]

use super::clevels::{ZSTD_CLEVEL_DEFAULT, ZSTD_DEFAULT_CPARAMETERS, ZSTD_MAX_CLEVEL};
use super::common::{highbit32, read16, read32, read64, write_le24, write_le32, CError, CResult};
use super::fse_compress::FseCTable;
use super::hist;
use super::huf_compress::{HufCompressTables, HufRepeat};
use super::zstd_compress_literals::{compress_literals, HufCTables};
use super::zstd_compress_sequences::{
    self as sequences, FseRepeat, DEFAULT_MAX_OFF, LL_DEFAULT_NORM, LL_DEFAULT_NORM_LOG, LL_FSE_LOG, MAX_LL, MAX_ML, MAX_OFF, MAX_SEQ, ML_DEFAULT_NORM, ML_DEFAULT_NORM_LOG, ML_FSE_LOG, OFF_FSE_LOG,
    OF_DEFAULT_NORM, OF_DEFAULT_NORM_LOG,
};
use super::zstd_decompress::ZstdError;
use super::zstd_double_fast::compress_block_double_fast;
use super::zstd_fast::compress_block_fast;
use super::zstd_presplit::split_block;

// From `lib/zstd.h` and `lib/common/zstd_internal.h`.
const ZSTD_MAGICNUMBER: u32 = 0xFD2FB528;
const ZSTD_WINDOWLOG_MAX: u32 = 31;
const ZSTD_WINDOWLOG_ABSOLUTEMIN: u32 = 10;
const ZSTD_HASHLOG_MIN: u32 = 6;
const ZSTD_BLOCKSIZE_MAX: usize = 1 << 17;
const ZSTD_TARGETLENGTH_MAX: i32 = ZSTD_BLOCKSIZE_MAX as i32;
const ZSTD_FRAMEHEADERSIZE_MAX: usize = 18;
const ZSTD_BLOCK_HEADER_SIZE: usize = 3;
const MIN_CBLOCK_SIZE: usize = 1 /* litCSize */ + 1 /* RLE or RAW */;
const MINMATCH: usize = 3;
const LONGNBSEQ: usize = 0x7F00;
const BT_RAW: u32 = 0;
const BT_RLE: u32 = 1;
const BT_COMPRESSED: u32 = 2;

/// `SymbolEncodingType_e`: how a table of a block comes.
pub(crate) const SET_BASIC: u32 = 0;
pub(crate) const SET_RLE: u32 = 1;
pub(crate) const SET_COMPRESSED: u32 = 2;
pub(crate) const SET_REPEAT: u32 = 3;

// From `lib/compress/zstd_compress_internal.h`.
pub(crate) const K_SEARCH_STRENGTH: u32 = 8;
pub(crate) const HASH_READ_SIZE: usize = 8;
/// The index of the first byte of a source: indices 0 and 1 are kept for "no match".
pub(crate) const ZSTD_WINDOW_START_INDEX: u32 = 2;
const ZSTD_REP_NUM: u32 = 3;
pub(crate) const REPCODE1_TO_OFFBASE: u32 = 1;
const REP_START_VALUE: [u32; 3] = [1, 4, 8];
const SUSPECT_UNCOMPRESSIBLE_LITERAL_RATIO: usize = 20;

/// `OFFSET_TO_OFFBASE`.
#[inline]
pub(crate) fn offset_to_offbase(offset: u32) -> u32 {
    offset + ZSTD_REP_NUM
}

/// `ZSTD_strategy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strategy {
    Fast = 1,
    Dfast = 2,
    Greedy = 3,
    Lazy = 4,
    Lazy2 = 5,
    Btlazy2 = 6,
    Btopt = 7,
    Btultra = 8,
    Btultra2 = 9,
}

/// `ZSTD_compressionParameters`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CompressionParameters {
    /// The largest match distance: a larger one compresses more and uses more memory.
    pub(crate) window_log: u32,
    /// For `ZSTD_dfast`, the size of the table of short matches.
    pub(crate) chain_log: u32,
    /// The size of the table of matches.
    pub(crate) hash_log: u32,
    /// For the strategies that are not ported: how hard to search.
    #[allow(dead_code)]
    pub(crate) search_log: u32,
    /// Matches shorter than this are not looked for.
    pub(crate) min_match: u32,
    /// For `ZSTD_fast`, the acceleration.
    pub(crate) target_length: u32,
    pub(crate) strategy: Strategy,
}

/// `ZSTD_COMPRESSBOUND`: the most bytes a frame can take for a source of `src_size` bytes.
/// It is what [`compress`] gives its output to start with, as the bytes depend on the room
/// there is in a few places.
pub fn compress_bound(src_size: usize) -> usize {
    src_size + (src_size >> 8) + if src_size < (128 << 10) { ((128 << 10) - src_size) >> 11 } else { 0 }
}

/// `ZSTD_adjustCParams_internal`, for a source of a known size and no dictionary: the
/// tables are not made larger than the source can use.
fn adjust_cparams_internal(mut c_par: CompressionParameters, src_size: u64) -> CompressionParameters {
    const MAX_WINDOW_RESIZE: u64 = 1 << (ZSTD_WINDOWLOG_MAX - 1);

    // Resize windowLog if the input is small enough, to use less memory.
    if src_size <= MAX_WINDOW_RESIZE {
        let t_size = src_size as u32;
        const HASH_SIZE_MIN: u32 = 1 << ZSTD_HASHLOG_MIN;
        let src_log = if t_size < HASH_SIZE_MIN { ZSTD_HASHLOG_MIN } else { highbit32(t_size - 1) + 1 };
        if c_par.window_log > src_log {
            c_par.window_log = src_log;
        }
    }
    {
        let dict_and_window_log = c_par.window_log;
        // `ZSTD_cycleLog`.
        let cycle_log = c_par.chain_log - u32::from(c_par.strategy as u32 >= Strategy::Btlazy2 as u32);
        if c_par.hash_log > dict_and_window_log + 1 {
            c_par.hash_log = dict_and_window_log + 1;
        }
        if cycle_log > dict_and_window_log {
            c_par.chain_log -= cycle_log - dict_and_window_log;
        }
    }

    if c_par.window_log < ZSTD_WINDOWLOG_ABSOLUTEMIN {
        c_par.window_log = ZSTD_WINDOWLOG_ABSOLUTEMIN; // minimum wlog required for valid frame header
    }

    // (The original then keeps the hash log of the strategies that search by rows under a
    // limit that no row of the table of levels reaches.)

    c_par
}

/// `ZSTD_getCParams_internal`, for a source of a known size and no dictionary: the
/// parameters of a level.
pub(crate) fn get_cparams_internal(compression_level: i32, src_size_hint: u64) -> CompressionParameters {
    let r_size = src_size_hint;
    let table_id = usize::from(r_size <= 256 * 1024) + usize::from(r_size <= 128 * 1024) + usize::from(r_size <= 16 * 1024);

    // Row.
    let row = if compression_level == 0 {
        ZSTD_CLEVEL_DEFAULT // 0 == default
    } else if compression_level < 0 {
        0 // entry 0 is baseline for fast mode
    } else if compression_level > ZSTD_MAX_CLEVEL {
        ZSTD_MAX_CLEVEL
    } else {
        compression_level
    };

    let (window_log, chain_log, hash_log, search_log, min_match, target_length, strategy) = ZSTD_DEFAULT_CPARAMETERS[table_id][row as usize];
    let mut cp = CompressionParameters { window_log, chain_log, hash_log, search_log, min_match, target_length, strategy };
    // Acceleration factor.
    if compression_level < 0 {
        // `ZSTD_minCLevel` is `-ZSTD_TARGETLENGTH_MAX`.
        let clamped_compression_level = compression_level.max(-ZSTD_TARGETLENGTH_MAX);
        cp.target_length = (-clamped_compression_level) as u32;
    }
    // Refine parameters based on srcSize.
    adjust_cparams_internal(cp, src_size_hint)
}

/// `ZSTD_minGain`: what a block has to save to be kept compressed.
pub(crate) fn min_gain(src_size: usize, strat: Strategy) -> usize {
    let minlog = if strat as u32 >= Strategy::Btultra as u32 { strat as u32 - 1 } else { 6 };
    (src_size >> minlog) + 2
}

/// `ZSTD_LLcode`: the code of a literals length.
fn ll_code(lit_length: u32) -> u32 {
    #[rustfmt::skip]
    const LL_CODE: [u8; 64] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21,
        22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23,
        24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    ];
    const LL_DELTA_CODE: u32 = 19;
    if lit_length > 63 {
        highbit32(lit_length) + LL_DELTA_CODE
    } else {
        LL_CODE[lit_length as usize] as u32
    }
}

/// `ZSTD_MLcode`: the code of a match length less `MINMATCH`.
fn ml_code(ml_base: u32) -> u32 {
    #[rustfmt::skip]
    const ML_CODE: [u8; 128] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        32, 32, 33, 33, 34, 34, 35, 35, 36, 36, 36, 36, 37, 37, 37, 37,
        38, 38, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39, 39,
        40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40,
        41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41,
        42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
        42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
    ];
    const ML_DELTA_CODE: u32 = 36;
    if ml_base > 127 {
        highbit32(ml_base) + ML_DELTA_CODE
    } else {
        ML_CODE[ml_base as usize] as u32
    }
}

/// `SeqDef`: a sequence as the block compressors leave it.
#[derive(Clone, Copy)]
pub(crate) struct SeqDef {
    /// The offset plus `ZSTD_REP_NUM`, or 1 to 3 for a repeat offset.
    pub(crate) off_base: u32,
    pub(crate) lit_length: u16,
    /// The match length less `MINMATCH`.
    pub(crate) ml_base: u16,
}

/// `ZSTD_longLengthType_e`: which of the two lengths of one sequence does not fit 16 bits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LongLengthType {
    None,
    LiteralLength,
    MatchLength,
}

/// `SeqStore_t`: the sequences and the literals of one block.
pub(crate) struct SeqStore {
    sequences: Vec<SeqDef>,
    lit: Vec<u8>,
    ll_code: Vec<u8>,
    ml_code: Vec<u8>,
    of_code: Vec<u8>,
    long_length_type: LongLengthType,
    /// The index of the sequence to apply the long length to.
    long_length_pos: u32,
}

impl SeqStore {
    fn new() -> SeqStore {
        SeqStore { sequences: Vec::new(), lit: Vec::new(), ll_code: Vec::new(), ml_code: Vec::new(), of_code: Vec::new(), long_length_type: LongLengthType::None, long_length_pos: 0 }
    }

    /// `ZSTD_resetSeqStore`.
    fn reset(&mut self) {
        self.lit.clear();
        self.sequences.clear();
        self.long_length_type = LongLengthType::None;
    }

    /// `ZSTD_storeSeq`: one sequence, with its `lit_length` literals from `src[literals..]`.
    #[inline]
    pub(crate) fn store_seq(&mut self, src: &[u8], literals: usize, lit_length: usize, off_base: u32, match_length: usize) {
        // Copy the literals.
        self.lit.extend_from_slice(&src[literals..literals + lit_length]);

        // `ZSTD_storeSeqOnly`.
        // Literal length.
        if lit_length > 0xFFFF {
            self.long_length_type = LongLengthType::LiteralLength;
            self.long_length_pos = self.sequences.len() as u32;
        }
        // Match length.
        let ml_base = match_length - MINMATCH;
        if ml_base > 0xFFFF {
            self.long_length_type = LongLengthType::MatchLength;
            self.long_length_pos = self.sequences.len() as u32;
        }
        self.sequences.push(SeqDef { off_base, lit_length: lit_length as u16, ml_base: ml_base as u16 });
    }

    /// `ZSTD_seqToCodes`: the codes of the lengths and offsets of the sequences.
    fn seq_to_codes(&mut self) {
        self.ll_code.clear();
        self.of_code.clear();
        self.ml_code.clear();
        for seq in &self.sequences {
            self.ll_code.push(ll_code(seq.lit_length as u32) as u8);
            self.of_code.push(highbit32(seq.off_base) as u8);
            self.ml_code.push(ml_code(seq.ml_base as u32) as u8);
        }
        if self.long_length_type == LongLengthType::LiteralLength {
            self.ll_code[self.long_length_pos as usize] = MAX_LL as u8;
        }
        if self.long_length_type == LongLengthType::MatchLength {
            self.ml_code[self.long_length_pos as usize] = MAX_ML as u8;
        }
    }
}

/// `ZSTD_window_t`. The source is one piece and there is no dictionary, so what the
/// original keeps as pointers comes to this: a byte of the source at `pos` has the index
/// `pos + ZSTD_WINDOW_START_INDEX`.
pub(crate) struct Window {
    /// Below this index is the dictionary; there is none, and this only moves up with
    /// `low_limit`.
    pub(crate) dict_limit: u32,
    /// Below this index no match is valid.
    pub(crate) low_limit: u32,
}

/// `ZSTD_MatchState_t`, for the strategies that are ported.
pub(crate) struct MatchState {
    pub(crate) window: Window,
    /// The index of the end of a dictionary: 0, there is none.
    loaded_dict_end: u32,
    /// The index from which to continue the table update.
    next_to_update: u32,
    pub(crate) hash_table: Vec<u32>,
    /// For `ZSTD_dfast`, the table of short matches.
    pub(crate) chain_table: Vec<u32>,
    pub(crate) cparams: CompressionParameters,
}

/// `ZSTD_getLowestPrefixIndex`: the lowest index a match may have for a block that ends at
/// `curr`.
pub(crate) fn get_lowest_prefix_index(ms: &MatchState, curr: u32, window_log: u32) -> u32 {
    let max_distance = 1u32 << window_log;
    let lowest_valid = ms.window.dict_limit;
    let within_window = if curr.wrapping_sub(lowest_valid) > max_distance { curr - max_distance } else { lowest_valid };
    let is_dictionary = ms.loaded_dict_end != 0;
    // When using a dictionary the entire dictionary is valid if a single byte of the
    // dictionary is within the window. It is invalidated as soon as the first byte of the
    // dictionary is out of the window.
    if is_dictionary {
        lowest_valid
    } else {
        within_window
    }
}

/// `ZSTD_count`: how many bytes at `p_in` are the same as at `p_match`, up to `p_in_limit`.
#[inline]
pub(crate) fn count(src: &[u8], mut p_in: usize, mut p_match: usize, p_in_limit: usize) -> usize {
    let p_start = p_in;
    // `pInLoopLimit` is `pInLimit - 7`.
    if p_in + 7 < p_in_limit {
        {
            let diff = read64(src, p_match) ^ read64(src, p_in);
            if diff != 0 {
                return (diff.trailing_zeros() >> 3) as usize;
            }
        }
        p_in += 8;
        p_match += 8;
        while p_in + 7 < p_in_limit {
            let diff = read64(src, p_match) ^ read64(src, p_in);
            if diff == 0 {
                p_in += 8;
                p_match += 8;
                continue;
            }
            p_in += (diff.trailing_zeros() >> 3) as usize;
            return p_in - p_start;
        }
    }
    if p_in + 3 < p_in_limit && read32(src, p_match) == read32(src, p_in) {
        p_in += 4;
        p_match += 4;
    }
    if p_in + 1 < p_in_limit && read16(src, p_match) == read16(src, p_in) {
        p_in += 2;
        p_match += 2;
    }
    if p_in < p_in_limit && src[p_match] == src[p_in] {
        p_in += 1;
    }
    p_in - p_start
}

const PRIME_4_BYTES: u32 = 2654435761;
const PRIME_5_BYTES: u64 = 889523592379;
const PRIME_6_BYTES: u64 = 227718039650203;
const PRIME_7_BYTES: u64 = 58295818150454627;
const PRIME_8_BYTES: u64 = 0xCF1BBCDCB7A56463;

/// `ZSTD_hashPtr`: the hash, of `h_bits` bits, of the `mls` bytes at `pos`.
#[inline]
pub(crate) fn hash_ptr(src: &[u8], pos: usize, h_bits: u32, mls: u32) -> usize {
    match mls {
        5 => ((read64(src, pos) << (64 - 40)).wrapping_mul(PRIME_5_BYTES) >> (64 - h_bits)) as usize,
        6 => ((read64(src, pos) << (64 - 48)).wrapping_mul(PRIME_6_BYTES) >> (64 - h_bits)) as usize,
        7 => ((read64(src, pos) << (64 - 56)).wrapping_mul(PRIME_7_BYTES) >> (64 - h_bits)) as usize,
        8 => (read64(src, pos).wrapping_mul(PRIME_8_BYTES) >> (64 - h_bits)) as usize,
        _ => (read32(src, pos).wrapping_mul(PRIME_4_BYTES) >> (32 - h_bits)) as usize,
    }
}

/// `ZSTD_fseCTables_t`.
#[derive(Clone)]
struct FseCTables {
    offcode_ctable: FseCTable,
    matchlength_ctable: FseCTable,
    litlength_ctable: FseCTable,
    offcode_repeat_mode: FseRepeat,
    matchlength_repeat_mode: FseRepeat,
    litlength_repeat_mode: FseRepeat,
}

/// `ZSTD_entropyCTables_t`.
#[derive(Clone)]
struct EntropyCTables {
    huf: HufCTables,
    fse: FseCTables,
}

/// `ZSTD_compressedBlockState_t`: what one block leaves for the next.
struct CompressedBlockState {
    entropy: EntropyCTables,
    rep: [u32; 3],
}

impl CompressedBlockState {
    /// A state as `ZSTD_reset_compressedBlockState` leaves it. (The original leaves the
    /// tables as they are, which are not read before they are written.)
    fn new() -> CompressedBlockState {
        CompressedBlockState {
            entropy: EntropyCTables {
                huf: HufCTables { ctable: [0; 257], repeat_mode: HufRepeat::None },
                fse: FseCTables {
                    offcode_ctable: FseCTable::new(OFF_FSE_LOG, MAX_OFF),
                    matchlength_ctable: FseCTable::new(ML_FSE_LOG, MAX_ML),
                    litlength_ctable: FseCTable::new(LL_FSE_LOG, MAX_LL),
                    offcode_repeat_mode: FseRepeat::None,
                    matchlength_repeat_mode: FseRepeat::None,
                    litlength_repeat_mode: FseRepeat::None,
                },
            },
            rep: REP_START_VALUE,
        }
    }
}

/// What `ZSTD_buildSequencesStatistics` answers with (`ZSTD_symbolEncodingTypeStats_t`).
struct SymbolEncodingTypeStats {
    ll_type: u32,
    off_type: u32,
    ml_type: u32,
    /// The size of the descriptions of the three tables.
    size: usize,
    /// The size of the last description of a table of its own.
    last_count_size: usize,
}

/// `ZSTD_buildSequencesStatistics`: the kinds of the three tables of the sequences, the
/// tables themselves in `next_entropy`, and what describes them in `dst`.
fn build_sequences_statistics(seq_store: &mut SeqStore, nb_seq: usize, prev_entropy: &FseCTables, next_entropy: &mut FseCTables, dst: &mut [u8], strategy: Strategy, count_workspace: &mut [u32; MAX_SEQ + 1]) -> CResult<SymbolEncodingTypeStats> {
    let mut op = 0usize;
    let mut stats = SymbolEncodingTypeStats { ll_type: 0, off_type: 0, ml_type: 0, size: 0, last_count_size: 0 };

    // Convert the lengths and distances into codes.
    seq_store.seq_to_codes();
    let of_code_table = &seq_store.of_code;
    let ll_code_table = &seq_store.ll_code;
    let ml_code_table = &seq_store.ml_code;

    // Build the table for the literal lengths.
    {
        let mut max = MAX_LL;
        let most_frequent = hist::count_simple(count_workspace, &mut max, &ll_code_table[..nb_seq]) as usize;
        next_entropy.litlength_repeat_mode = prev_entropy.litlength_repeat_mode;
        stats.ll_type = sequences::select_encoding_type(&mut next_entropy.litlength_repeat_mode, most_frequent, nb_seq, LL_DEFAULT_NORM_LOG, true, strategy);
        {
            let count_size = sequences::build_ctable(
                &mut dst[op..],
                &mut next_entropy.litlength_ctable,
                LL_FSE_LOG,
                stats.ll_type,
                count_workspace,
                max,
                ll_code_table,
                nb_seq,
                &LL_DEFAULT_NORM,
                LL_DEFAULT_NORM_LOG,
                MAX_LL,
                &prev_entropy.litlength_ctable,
            )?;
            if stats.ll_type == SET_COMPRESSED {
                stats.last_count_size = count_size;
            }
            op += count_size;
        }
    }
    // Build the table for the offsets.
    {
        let mut max = MAX_OFF;
        let most_frequent = hist::count_simple(count_workspace, &mut max, &of_code_table[..nb_seq]) as usize;
        // We can only use the basic table if max <= DefaultMaxOff, otherwise the offsets
        // are too large.
        let default_policy = max <= DEFAULT_MAX_OFF;
        next_entropy.offcode_repeat_mode = prev_entropy.offcode_repeat_mode;
        stats.off_type = sequences::select_encoding_type(&mut next_entropy.offcode_repeat_mode, most_frequent, nb_seq, OF_DEFAULT_NORM_LOG, default_policy, strategy);
        {
            let count_size = sequences::build_ctable(
                &mut dst[op..],
                &mut next_entropy.offcode_ctable,
                OFF_FSE_LOG,
                stats.off_type,
                count_workspace,
                max,
                of_code_table,
                nb_seq,
                &OF_DEFAULT_NORM,
                OF_DEFAULT_NORM_LOG,
                DEFAULT_MAX_OFF,
                &prev_entropy.offcode_ctable,
            )?;
            if stats.off_type == SET_COMPRESSED {
                stats.last_count_size = count_size;
            }
            op += count_size;
        }
    }
    // Build the table for the match lengths.
    {
        let mut max = MAX_ML;
        let most_frequent = hist::count_simple(count_workspace, &mut max, &ml_code_table[..nb_seq]) as usize;
        next_entropy.matchlength_repeat_mode = prev_entropy.matchlength_repeat_mode;
        stats.ml_type = sequences::select_encoding_type(&mut next_entropy.matchlength_repeat_mode, most_frequent, nb_seq, ML_DEFAULT_NORM_LOG, true, strategy);
        {
            let count_size = sequences::build_ctable(
                &mut dst[op..],
                &mut next_entropy.matchlength_ctable,
                ML_FSE_LOG,
                stats.ml_type,
                count_workspace,
                max,
                ml_code_table,
                nb_seq,
                &ML_DEFAULT_NORM,
                ML_DEFAULT_NORM_LOG,
                MAX_ML,
                &prev_entropy.matchlength_ctable,
            )?;
            if stats.ml_type == SET_COMPRESSED {
                stats.last_count_size = count_size;
            }
            op += count_size;
        }
    }
    stats.size = op;
    Ok(stats)
}

/// `ZSTD_isRLE`: whether the block is one byte over and over. (The original compares
/// eight bytes at a time.)
fn is_rle(src: &[u8]) -> bool {
    let value = src[0];
    src.iter().all(|&b| b == value)
}

/// `ZSTD_CCtx`, as far as one call of `ZSTD_compress` uses it.
struct CCtx {
    /// `appliedParams.cParams`.
    cparams: CompressionParameters,
    block_size_max: usize,
    is_first_block: bool,
    /// `stage == ZSTDcs_ending`: the last block is written.
    stage_ending: bool,
    consumed_src_size: u64,
    produced_c_size: u64,
    prev_cblock: Box<CompressedBlockState>,
    next_cblock: Box<CompressedBlockState>,
    match_state: MatchState,
    seq_store: SeqStore,
    /// The work area of the entropy stage: the counts of the sequences' codes, then what
    /// the Huffman compressor keeps.
    count: [u32; MAX_SEQ + 1],
    huf_workspace: Box<HufCompressTables>,
}

impl CCtx {
    /// A context as `ZSTD_compressBegin_internal` leaves it (`ZSTD_resetCCtx_internal`,
    /// `ZSTD_reset_matchState`, `ZSTD_reset_compressedBlockState`), for a source of
    /// `pledged_src_size` bytes.
    fn new(cparams: CompressionParameters, pledged_src_size: u64) -> CCtx {
        let window_size = 1usize.max((1u64 << cparams.window_log).min(pledged_src_size) as usize);
        let block_size = ZSTD_BLOCKSIZE_MAX.min(window_size);
        // `ZSTD_allocateChainTable`: every strategy but the fast one has one.
        let chain_size = if cparams.strategy != Strategy::Fast { 1usize << cparams.chain_log } else { 0 };
        let h_size = 1usize << cparams.hash_log;
        CCtx {
            cparams,
            block_size_max: block_size,
            is_first_block: true,
            stage_ending: false,
            consumed_src_size: 0,
            produced_c_size: 0,
            prev_cblock: Box::new(CompressedBlockState::new()),
            next_cblock: Box::new(CompressedBlockState::new()),
            match_state: MatchState {
                // `ZSTD_window_init`.
                window: Window { dict_limit: ZSTD_WINDOW_START_INDEX, low_limit: ZSTD_WINDOW_START_INDEX },
                loaded_dict_end: 0,
                next_to_update: ZSTD_WINDOW_START_INDEX,
                hash_table: vec![0; h_size],
                chain_table: vec![0; chain_size],
                cparams,
            },
            seq_store: SeqStore::new(),
            count: [0; MAX_SEQ + 1],
            huf_workspace: Box::new(HufCompressTables::new()),
        }
    }

    /// `ZSTD_literalsCompressionIsDisabled`: the fast strategy with an acceleration leaves
    /// the literals as they are.
    fn literals_compression_is_disabled(&self) -> bool {
        self.cparams.strategy == Strategy::Fast && self.cparams.target_length > 0
    }

    /// `ZSTD_entropyCompressSeqStore_internal`: the literals and the sequences of the block
    /// in the sequence store, as the two sections of a compressed block.
    fn entropy_compress_seq_store_internal(&mut self, dst: &mut [u8]) -> CResult<usize> {
        let strategy = self.cparams.strategy;
        let disable_literal_compression = self.literals_compression_is_disabled();
        let seq_store = &mut self.seq_store;
        let prev_entropy = &self.prev_cblock.entropy;
        let next_entropy = &mut self.next_cblock.entropy;
        let nb_seq = seq_store.sequences.len();
        let oend = dst.len();
        let mut op = 0usize;
        let last_count_size;

        // Compress the literals.
        {
            let lit_size = seq_store.lit.len();
            let num_sequences = nb_seq;
            // Base suspicion of uncompressibility on ratio of literals to sequences.
            let suspect_uncompressible = (num_sequences == 0) || (lit_size / num_sequences >= SUSPECT_UNCOMPRESSIBLE_LITERAL_RATIO);

            let c_size = compress_literals(dst, &seq_store.lit, &mut self.huf_workspace, &prev_entropy.huf, &mut next_entropy.huf, strategy, disable_literal_compression, suspect_uncompressible)?;
            op += c_size;
        }

        // Sequences header.
        if (oend - op) < 3 /* max nbSeq Size */ + 1 /* seqHead */ {
            return Err(CError::DstSizeTooSmall); // can't fit the sequences header in the output
        }
        if nb_seq < 128 {
            dst[op] = nb_seq as u8;
            op += 1;
        } else if nb_seq < LONGNBSEQ {
            dst[op] = ((nb_seq >> 8) + 0x80) as u8;
            dst[op + 1] = nb_seq as u8;
            op += 2;
        } else {
            dst[op] = 0xFF;
            dst[op + 1..op + 3].copy_from_slice(&((nb_seq - LONGNBSEQ) as u16).to_le_bytes());
            op += 3;
        }
        if nb_seq == 0 {
            // Copy the entropy tables.
            next_entropy.fse = prev_entropy.fse.clone();
            return Ok(op);
        }
        {
            let seq_head = op;
            op += 1;
            // Build the stats for the sequences.
            let stats = build_sequences_statistics(seq_store, nb_seq, &prev_entropy.fse, &mut next_entropy.fse, &mut dst[op..], strategy, &mut self.count)?;
            dst[seq_head] = ((stats.ll_type << 6) + (stats.off_type << 4) + (stats.ml_type << 2)) as u8;
            last_count_size = stats.last_count_size;
            op += stats.size;
        }

        {
            let bitstream_size =
                sequences::encode_sequences(&mut dst[op..], &next_entropy.fse.matchlength_ctable, &seq_store.ml_code, &next_entropy.fse.offcode_ctable, &seq_store.of_code, &next_entropy.fse.litlength_ctable, &seq_store.ll_code, &seq_store.sequences)?;
            op += bitstream_size;
            // zstd versions <= 1.3.4 mistakenly report corruption when FSE_readNCount()
            // receives a buffer < 4 bytes. This can happen when the last set_compressed
            // table present is 2 bytes and the bitstream is only one byte. In this
            // exceedingly rare case, the block is given up and sent uncompressed.
            if last_count_size != 0 && (last_count_size + bitstream_size) < 4 {
                // lastCountSize >= 2 && bitstreamSize > 0 ==> lastCountSize == 3
                return Ok(0);
            }
        }

        Ok(op)
    }

    /// `ZSTD_entropyCompressSeqStore`: as above, or 0 when the block is better left as it
    /// is. `block_size` is the size of the block's source.
    fn entropy_compress_seq_store(&mut self, dst: &mut [u8], block_size: usize) -> CResult<usize> {
        let dst_capacity = dst.len();
        let c_size = self.entropy_compress_seq_store_internal(dst);
        match c_size {
            Ok(0) => Ok(0),
            // When the block is not compressible, the room can run out before the end; the
            // block is then sent as it is, for which there is room.
            Err(CError::DstSizeTooSmall) if block_size <= dst_capacity => Ok(0),
            Err(error) => Err(error),
            Ok(c_size) => {
                // Check compressibility.
                let max_c_size = block_size - min_gain(block_size, self.cparams.strategy);
                if c_size >= max_c_size {
                    return Ok(0); // block not compressed
                }
                Ok(c_size)
            }
        }
    }

    /// `ZSTD_buildSeqStore`: the sequences of the block `src[ip..ip + src_size]`. `false`
    /// (`ZSTDbss_noCompress`) for a block too small to be worth it.
    fn build_seq_store(&mut self, src: &[u8], ip: usize, src_size: usize) -> bool {
        let ms = &mut self.match_state;
        // Assert that we have correctly flushed the ctx params into the ms's copy.
        if src_size < MIN_CBLOCK_SIZE + ZSTD_BLOCK_HEADER_SIZE + 1 + 1 {
            return false; // don't even attempt compression below a certain srcSize
        }
        self.seq_store.reset();

        // Limited update after a very long match.
        {
            let curr = (ip + ZSTD_WINDOW_START_INDEX as usize) as u32;
            if curr > ms.next_to_update + 384 {
                ms.next_to_update = curr - 192u32.min(curr - ms.next_to_update - 384);
            }
        }

        // Select and store sequences.
        {
            self.next_cblock.rep = self.prev_cblock.rep;
            // `ZSTD_selectBlockCompressor`.
            let last_ll_size = match ms.cparams.strategy {
                Strategy::Fast => compress_block_fast(ms, &mut self.seq_store, &mut self.next_cblock.rep, src, ip, src_size),
                _ => compress_block_double_fast(ms, &mut self.seq_store, &mut self.next_cblock.rep, src, ip, src_size),
            };
            // `ZSTD_storeLastLiterals`.
            let last_literals = ip + src_size - last_ll_size;
            self.seq_store.lit.extend_from_slice(&src[last_literals..last_literals + last_ll_size]);
        }
        true
    }

    /// `ZSTD_compressBlock_internal`: the block `src[ip..ip + src_size]`, compressed, with
    /// no header. 0 if it is better left as it is; 1 if it is one byte over and over, which
    /// is then in `dst[0]`.
    fn compress_block_internal(&mut self, dst: &mut [u8], src: &[u8], ip: usize, src_size: usize, frame: bool) -> CResult<usize> {
        // This is the upper bound for the length of an rle block. This isn't the actual
        // upper bound; finding the real threshold needs further investigation.
        const RLE_MAX_LENGTH: usize = 25;
        let c_size;

        if !self.build_seq_store(src, ip, src_size) {
            c_size = Ok(0);
        } else {
            // Encode the sequences into the block.
            let mut result = self.entropy_compress_seq_store(dst, src_size);

            if frame &&
                // We don't want to emit our first block as a RLE even if it qualifies because
                // doing so will cause the decoder (cli only) to throw a "should consume all
                // input" error. This is only an issue for zstd <= v1.4.3.
                !self.is_first_block && matches!(result, Ok(size) if size < RLE_MAX_LENGTH) && is_rle(&src[ip..ip + src_size])
            {
                result = Ok(1);
                dst[0] = src[ip];
            }
            c_size = result;
        }

        // out:
        if matches!(c_size, Ok(size) if size > 1) {
            // `ZSTD_blockState_confirmRepcodesAndEntropyTables`.
            std::mem::swap(&mut self.prev_cblock, &mut self.next_cblock);
        }
        // We check that dictionaries have offset codes available for the first block. After
        // the first block, the offcode table might not have large enough codes to represent
        // the offsets in the data.
        if self.prev_cblock.entropy.fse.offcode_repeat_mode == FseRepeat::Valid {
            self.prev_cblock.entropy.fse.offcode_repeat_mode = FseRepeat::Check;
        }

        c_size
    }

    /// `ZSTD_optimalBlockSize`: how much of the rest of the source the next block takes.
    fn optimal_block_size(&self, src: &[u8], block_size_max: usize, savings: i64) -> usize {
        // Split level based on compression strategy, from `fast` to `btultra2`.
        const SPLIT_LEVELS: [i32; 10] = [0, 0, 1, 2, 2, 3, 3, 4, 4, 4];
        let src_size = src.len();
        // Note: conservatively only split full blocks (128 KB) currently.
        if src_size < 128 * 1024 || block_size_max < 128 * 1024 {
            return src_size.min(block_size_max);
        }
        // Do not split incompressible data though: require verified savings to allow
        // pre-splitting. Note: as a consequence, the first full block is not split.
        if savings < 3 {
            return 128 * 1024;
        }
        // (The level of the splitter is left for the strategy to say, `ZSTD_c_splitAfterSequences`
        // not being set.)
        let split_level = SPLIT_LEVELS[self.cparams.strategy as usize];
        split_block(&src[..block_size_max], split_level)
    }

    /// `ZSTD_compress_frameChunk`: the source as blocks.
    fn compress_frame_chunk(&mut self, dst: &mut [u8], src: &[u8]) -> CResult<usize> {
        let block_size_max = self.block_size_max;
        let mut remaining = src.len();
        let mut ip = 0usize;
        let mut op = 0usize;
        let mut dst_capacity = dst.len();
        let max_dist = 1u32 << self.cparams.window_log;
        let mut savings = self.consumed_src_size as i64 - self.produced_c_size as i64;

        while remaining != 0 {
            let block_size = self.optimal_block_size(&src[ip..], block_size_max, savings);
            let last_block = u32::from(block_size == remaining);

            if dst_capacity < ZSTD_BLOCK_HEADER_SIZE + MIN_CBLOCK_SIZE + 1 {
                return Err(CError::DstSizeTooSmall); // not enough space to store compressed block
            }

            // (`ZSTD_overflowCorrectIfNeeded`: the indices of a source this port takes
            // stay far from where they would need to be brought down.
            // `ZSTD_checkDictValidity`: there is no dictionary.)
            // `ZSTD_window_enforceMaxDist`.
            {
                let window = &mut self.match_state.window;
                let block_end_idx = (ip + ZSTD_WINDOW_START_INDEX as usize) as u32;
                if block_end_idx > max_dist {
                    let new_low_limit = block_end_idx - max_dist;
                    if window.low_limit < new_low_limit {
                        window.low_limit = new_low_limit;
                    }
                    if window.dict_limit < window.low_limit {
                        window.dict_limit = window.low_limit;
                    }
                }
            }

            // Ensure hash/chain table insertion resumes no sooner than lowlimit.
            if self.match_state.next_to_update < self.match_state.window.low_limit {
                self.match_state.next_to_update = self.match_state.window.low_limit;
            }

            {
                let block_dst = &mut dst[op..op + dst_capacity];
                let mut c_size = self.compress_block_internal(&mut block_dst[ZSTD_BLOCK_HEADER_SIZE..], src, ip, block_size, true)?;

                if c_size == 0 {
                    // Block is not compressible. `ZSTD_noCompressBlock`.
                    let c_block_header24 = last_block + (BT_RAW << 1) + ((block_size as u32) << 3);
                    if block_size + ZSTD_BLOCK_HEADER_SIZE > dst_capacity {
                        return Err(CError::DstSizeTooSmall); // dst buf too small for uncompressed block
                    }
                    write_le24(block_dst, 0, c_block_header24);
                    block_dst[ZSTD_BLOCK_HEADER_SIZE..ZSTD_BLOCK_HEADER_SIZE + block_size].copy_from_slice(&src[ip..ip + block_size]);
                    c_size = ZSTD_BLOCK_HEADER_SIZE + block_size;
                } else {
                    let c_block_header = if c_size == 1 { last_block + (BT_RLE << 1) + ((block_size as u32) << 3) } else { last_block + (BT_COMPRESSED << 1) + ((c_size as u32) << 3) };
                    write_le24(block_dst, 0, c_block_header);
                    c_size += ZSTD_BLOCK_HEADER_SIZE;
                }

                // note: do not merge block reductions (for example the size of the header) into `savings`
                savings += block_size as i64 - c_size as i64;

                ip += block_size;
                remaining -= block_size;
                op += c_size;
                dst_capacity -= c_size;
                self.is_first_block = false;
            }
        }

        if op > 0 {
            self.stage_ending = true;
        }
        Ok(op)
    }

    /// `ZSTD_writeFrameHeader`, with the size of the content, no checksum and no
    /// dictionary.
    fn write_frame_header(&self, dst: &mut [u8], pledged_src_size: u64) -> CResult<usize> {
        let window_size = 1u32 << self.cparams.window_log;
        let single_segment = window_size as u64 >= pledged_src_size;
        let window_log_byte = ((self.cparams.window_log - ZSTD_WINDOWLOG_ABSOLUTEMIN) << 3) as u8;
        let fcs_code = u32::from(pledged_src_size >= 256) + u32::from(pledged_src_size >= 65536 + 256) + u32::from(pledged_src_size >= 0xFFFFFFFF);
        let frame_header_description_byte = ((u32::from(single_segment) << 5) + (fcs_code << 6)) as u8;
        let mut pos = 0usize;

        if dst.len() < ZSTD_FRAMEHEADERSIZE_MAX {
            return Err(CError::DstSizeTooSmall); // dst buf is too small to fit worst-case frame header size
        }
        write_le32(dst, 0, ZSTD_MAGICNUMBER);
        pos += 4;
        dst[pos] = frame_header_description_byte;
        pos += 1;
        if !single_segment {
            dst[pos] = window_log_byte;
            pos += 1;
        }
        match fcs_code {
            0 => {
                if single_segment {
                    dst[pos] = pledged_src_size as u8;
                    pos += 1;
                }
            }
            1 => {
                dst[pos..pos + 2].copy_from_slice(&((pledged_src_size - 256) as u16).to_le_bytes());
                pos += 2;
            }
            2 => {
                write_le32(dst, pos, pledged_src_size as u32);
                pos += 4;
            }
            _ => {
                dst[pos..pos + 8].copy_from_slice(&pledged_src_size.to_le_bytes());
                pos += 8;
            }
        }
        Ok(pos)
    }

    /// `ZSTD_compressEnd_public` after `ZSTD_compressBegin_internal`: the whole frame.
    fn compress_end(&mut self, dst: &mut [u8], src: &[u8]) -> CResult<usize> {
        // `ZSTD_compressContinue_internal`, for a frame and its last chunk.
        let fh_size = self.write_frame_header(dst, src.len() as u64)?;
        let mut c_size = fh_size;
        if !src.is_empty() {
            // `ZSTD_window_update`: the source starts at index `ZSTD_WINDOW_START_INDEX`,
            // and is not what came before, there being nothing before.
            self.match_state.next_to_update = self.match_state.window.dict_limit;

            let chunk_size = self.compress_frame_chunk(&mut dst[fh_size..], src)?;
            self.consumed_src_size += src.len() as u64;
            self.produced_c_size += (chunk_size + fh_size) as u64;
            c_size += chunk_size;
        }

        // `ZSTD_writeEpilogue`.
        if !self.stage_ending {
            // Write one last empty block, make it the "last" block.
            let c_block_header24 = 1 /* last block */ + (BT_RAW << 1);
            if dst.len() - c_size < 3 {
                return Err(CError::DstSizeTooSmall); // no room for epilogue
            }
            write_le24(dst, c_size, c_block_header24);
            c_size += ZSTD_BLOCK_HEADER_SIZE;
        }
        Ok(c_size)
    }
}

/// `ZSTD_compress`: `src` as one Zstandard frame, at `level`.
///
/// The frame is byte for byte the one libzstd v1.5.7 gives for
/// `ZSTD_compress(dst, ZSTD_compressBound(n), src, n, level)`. Level 0 is the default
/// level, 3; negative levels are faster and compress less.
///
/// A level is refused when, for a source of this size, the original takes a strategy that
/// is not ported (see the top of this file): levels 1 to 3 and the negative levels are
/// always taken. A source of 2 GB or more is refused too.
pub fn compress(src: &[u8], level: i32) -> Result<Vec<u8>, ZstdError> {
    if src.len() >= 1usize << 31 {
        return Err(ZstdError("Source too large to compress"));
    }
    let cparams = get_cparams_internal(level, src.len() as u64);
    if !level_is_ported(level, src.len()) {
        return Err(ZstdError("Compression level not ported for a source of this size"));
    }
    let mut dst = vec![0u8; compress_bound(src.len())];
    let mut cctx = CCtx::new(cparams, src.len() as u64);
    match cctx.compress_end(&mut dst, src) {
        Ok(size) => {
            dst.truncate(size);
            Ok(dst)
        }
        // (An output of `compress_bound` bytes has room for any source.)
        Err(_) => Err(ZstdError("Compression failed")),
    }
}

/// Whether [`compress`] takes `level` for a source of `src_size` bytes: whether the
/// strategy the original has for them is one of the two that are ported.
pub fn level_is_ported(level: i32, src_size: usize) -> bool {
    matches!(get_cparams_internal(level, src_size as u64).strategy, Strategy::Fast | Strategy::Dfast)
}

#[cfg(test)]
mod tests {
    use super::super::zstd_decompress::decompress;
    use super::*;

    fn noise(len: usize, mut state: u64, mask: u8) -> Vec<u8> {
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (state >> 56) as u8 & mask
            })
            .collect()
    }

    #[test]
    fn nothing_is_a_frame_of_nine_bytes() {
        // The magic number, a header that says "one segment, of no bytes", and one last,
        // empty block.
        assert_eq!(compress(&[], 3).unwrap(), [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x00, 0x01, 0x00, 0x00]);
        assert_eq!(compress_bound(0), 64);
        assert_eq!(compress_bound(1 << 20), (1 << 20) + (1 << 12));
    }

    #[test]
    fn frames_decode_to_their_sources() {
        let mut text = Vec::new();
        while text.len() < 300_000 {
            let n = text.len();
            text.extend_from_slice(format!("line {n} of a text that says much the same again and again, {}\n", n % 7).as_bytes());
        }
        let sources = [vec![7u8], vec![0u8; 5000], noise(70_000, 1, 0xFF), noise(200_000, 2, 0x0F), text.clone(), [noise(150_000, 3, 0xFF), text].concat()];
        for source in &sources {
            for level in [-100, -1, 0, 1, 2, 3] {
                let frame = compress(source, level).unwrap();
                assert!(frame.len() <= compress_bound(source.len()));
                assert_eq!(decompress(&frame, source.len()).as_deref(), Ok(&source[..]), "{} bytes at level {level}", source.len());
            }
        }
        // What compresses does.
        assert!(compress(&sources[1], 1).unwrap().len() < 30);
        assert!(compress(&sources[4], 3).unwrap().len() < sources[4].len() / 5);
    }

    #[test]
    fn levels_of_other_strategies_are_refused() {
        for size in [0, 100, 16 << 10, (16 << 10) + 1, 128 << 10, (128 << 10) + 1, 256 << 10, (256 << 10) + 1, 5 << 20] {
            for level in [-131072, -5, -1, 0, 1, 2, 3] {
                assert!(level_is_ported(level, size), "{level} for {size}");
            }
            // Level 4 is the double-fast strategy for some sizes and the greedy one for others.
            assert_eq!(level_is_ported(4, size), size > 256 << 10 || (size > 16 << 10 && size <= 128 << 10), "{size}");
            for level in [5, 9, 19, 22, 100] {
                assert!(!level_is_ported(level, size), "{level} for {size}");
            }
        }
        assert_eq!(compress(&[1, 2, 3], 19), Err(ZstdError("Compression level not ported for a source of this size")));
    }

    #[test]
    fn the_parameters_of_a_level_follow_the_size_of_the_source() {
        // Level 3 for a large source, as the table has it.
        let p = get_cparams_internal(3, 10 << 20);
        assert_eq!((p.window_log, p.chain_log, p.hash_log, p.min_match, p.target_length, p.strategy), (21, 16, 17, 5, 0, Strategy::Dfast));
        // For a hundred bytes: the tables shrink with the window, which is then set to the
        // least a frame can say.
        let p = get_cparams_internal(3, 100);
        assert_eq!((p.window_log, p.chain_log, p.hash_log, p.min_match, p.strategy), (10, 7, 8, 4, Strategy::Dfast));
        // A negative level is the fast strategy with the level as its acceleration.
        let p = get_cparams_internal(-7, 1 << 20);
        assert_eq!((p.window_log, p.hash_log, p.min_match, p.target_length, p.strategy), (19, 13, 6, 7, Strategy::Fast));
        assert_eq!(get_cparams_internal(-1 << 20, 1 << 20).target_length, 1 << 17);
    }

    /// No source the comparison makes gives a block of 0x7F00 sequences or more, whose number
    /// is written the long way: here the sequences are made by hand, and the block is
    /// decoded by the port of the reference decoder.
    #[test]
    fn a_block_of_very_many_sequences_decodes() {
        const SEQUENCES: usize = 32700;
        let mut expected = Vec::new();
        let cparams = get_cparams_internal(3, SEQUENCES as u64 * 4);
        let mut cctx = CCtx::new(cparams, SEQUENCES as u64 * 4);
        for i in 0..SEQUENCES {
            // One literal, then three more of it: a match at a distance of one.
            let byte = [(i % 251) as u8];
            cctx.seq_store.store_seq(&byte, 0, 1, offset_to_offbase(1), 3);
            expected.extend_from_slice(&[byte[0]; 4]);
        }
        const { assert!(SEQUENCES >= LONGNBSEQ) };
        let mut frame = vec![0u8; compress_bound(expected.len())];
        let header = cctx.write_frame_header(&mut frame, expected.len() as u64).unwrap();
        let size = cctx.entropy_compress_seq_store(&mut frame[header + 3..], expected.len()).unwrap();
        assert!(size > 1);
        write_le24(&mut frame, header, 1 + (BT_COMPRESSED << 1) + ((size as u32) << 3));
        frame.truncate(header + 3 + size);
        assert_eq!(decompress(&frame, expected.len()).as_deref(), Ok(&expected[..]));
    }
}
