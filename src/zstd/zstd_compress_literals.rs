//! The literals section of a block: a port of `lib/compress/zstd_compress_literals.c` of
//! zstd v1.5.7.

use super::common::{write_le16, write_le24, write_le32, CError, CResult};
use super::huf_compress::{self, HufCTable, HufCompressTables, HufNbStreams, HufRepeat, HUF_SYMBOLVALUE_MAX};
use super::zstd_compress::{min_gain, Strategy, SET_BASIC, SET_COMPRESSED, SET_REPEAT, SET_RLE};

/// `LitHufLog`.
const LIT_HUF_LOG: u32 = 11;

/// `ZSTD_hufCTables_t`.
#[derive(Clone)]
pub(crate) struct HufCTables {
    pub(crate) ctable: HufCTable,
    pub(crate) repeat_mode: HufRepeat,
}

/// `ZSTD_noCompressLiterals`: the literals as they are, behind their size.
pub(crate) fn no_compress_literals(dst: &mut [u8], src: &[u8]) -> CResult<usize> {
    let src_size = src.len();
    let fl_size = 1 + usize::from(src_size > 31) + usize::from(src_size > 4095);

    if src_size + fl_size > dst.len() {
        return Err(CError::DstSizeTooSmall);
    }

    match fl_size {
        1 => dst[0] = (SET_BASIC + ((src_size as u32) << 3)) as u8, // 2 - 1 - 5
        2 => write_le16(dst, 0, (SET_BASIC + (1 << 2) + ((src_size as u32) << 4)) as u16), // 2 - 2 - 12
        _ => write_le32(dst, 0, SET_BASIC + (3 << 2) + ((src_size as u32) << 4)), // 2 - 2 - 20
    }

    dst[fl_size..fl_size + src_size].copy_from_slice(src);
    Ok(src_size + fl_size)
}

/// `allBytesIdentical`.
fn all_bytes_identical(src: &[u8]) -> bool {
    let b = src[0];
    src[1..].iter().all(|&x| x == b)
}

/// `ZSTD_compressRleLiteralsBlock`: one byte and how many times it comes.
pub(crate) fn compress_rle_literals_block(dst: &mut [u8], src: &[u8]) -> usize {
    let src_size = src.len();
    let fl_size = 1 + usize::from(src_size > 31) + usize::from(src_size > 4095);

    match fl_size {
        1 => dst[0] = (SET_RLE + ((src_size as u32) << 3)) as u8, // 2 - 1 - 5
        2 => write_le16(dst, 0, (SET_RLE + (1 << 2) + ((src_size as u32) << 4)) as u16), // 2 - 2 - 12
        _ => write_le32(dst, 0, SET_RLE + (3 << 2) + ((src_size as u32) << 4)), // 2 - 2 - 20
    }

    dst[fl_size] = src[0];
    fl_size + 1
}

/// `ZSTD_minLiteralsToCompress`: the fewest literals that are worth a try at compression,
/// which depends on the strategy and on whether there is a table from before.
fn min_literals_to_compress(strategy: Strategy, huf_repeat: HufRepeat) -> usize {
    // btultra2: min 8 bytes; then 2x larger for each successive compression strategy;
    // max threshold 64 bytes.
    let shift = (9 - strategy as i32).min(3);
    if huf_repeat == HufRepeat::Valid {
        6
    } else {
        8usize << shift
    }
}

/// `ZSTD_compressLiterals`: the literals section, Huffman coded where that is smaller.
/// `next_huf` becomes the table to carry to the next block.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_literals(
    dst: &mut [u8],
    src: &[u8],
    entropy_workspace: &mut HufCompressTables,
    prev_huf: &HufCTables,
    next_huf: &mut HufCTables,
    strategy: Strategy,
    disable_literal_compression: bool,
    suspect_uncompressible: bool,
) -> CResult<usize> {
    let src_size = src.len();
    let dst_capacity = dst.len();
    let lh_size = 3 + usize::from(src_size >= 1024) + usize::from(src_size >= 16 * 1024);
    let mut single_stream = src_size < 256;
    let mut h_type = SET_COMPRESSED;
    let c_lit_size;

    // Prepare nextEntropy assuming reusing the existing table.
    *next_huf = prev_huf.clone();

    if disable_literal_compression {
        return no_compress_literals(dst, src);
    }

    // If too small, don't even attempt compression (speed opt).
    if src_size < min_literals_to_compress(strategy, prev_huf.repeat_mode) {
        return no_compress_literals(dst, src);
    }

    if dst_capacity < lh_size + 1 {
        return Err(CError::DstSizeTooSmall); // not enough space for compression
    }
    {
        let mut repeat = prev_huf.repeat_mode;
        let prefer_repeat = (strategy as u32) < (Strategy::Lazy as u32) && src_size <= 1024;
        if repeat == HufRepeat::Valid && lh_size == 3 {
            single_stream = true;
        }
        let nb_streams = if single_stream { HufNbStreams::Single } else { HufNbStreams::Four };
        let result = huf_compress::compress_repeat(&mut dst[lh_size..], src, HUF_SYMBOLVALUE_MAX, LIT_HUF_LOG, nb_streams, entropy_workspace, &mut next_huf.ctable, &mut repeat, prefer_repeat, suspect_uncompressible);
        if repeat != HufRepeat::None {
            // Reused the existing table.
            h_type = SET_REPEAT;
        }
        let min_gain = min_gain(src_size, strategy);
        match result {
            Ok(size) if size != 0 && size < src_size - min_gain => c_lit_size = size,
            _ => {
                *next_huf = prev_huf.clone();
                return no_compress_literals(dst, src);
            }
        }
    }
    if c_lit_size == 1 {
        // A return value of 1 signals that the alphabet consists of a single symbol.
        // However, in some rare circumstances, it could be the compressed size (a single
        // byte). For that outcome to have a chance to happen, it's necessary that
        // srcSize < 8 (note that it's also necessary to avoid generating statistics).
        // Therefore, in such a case, actively check that all bytes are identical.
        if src_size >= 8 || all_bytes_identical(src) {
            *next_huf = prev_huf.clone();
            return Ok(compress_rle_literals_block(dst, src));
        }
    }

    if h_type == SET_COMPRESSED {
        // Using a newly constructed table.
        next_huf.repeat_mode = HufRepeat::Check;
    }

    // Build the header.
    match lh_size {
        3 => {
            // 2 - 2 - 10 - 10
            let lhc = h_type + (u32::from(!single_stream) << 2) + ((src_size as u32) << 4) + ((c_lit_size as u32) << 14);
            write_le24(dst, 0, lhc);
        }
        4 => {
            // 2 - 2 - 14 - 14
            let lhc = h_type + (2 << 2) + ((src_size as u32) << 4) + ((c_lit_size as u32) << 18);
            write_le32(dst, 0, lhc);
        }
        _ => {
            // 2 - 2 - 18 - 18
            let lhc = h_type + (3 << 2) + ((src_size as u32) << 4) + ((c_lit_size as u32) << 22);
            write_le32(dst, 0, lhc);
            dst[4] = (c_lit_size >> 10) as u8;
        }
    }
    Ok(lh_size + c_lit_size)
}
