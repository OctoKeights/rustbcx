//! Zstandard, for `.ktx2` files whose levels are supercompressed with it.
//!
//! - [`zstd_compress`]: bytes to a frame ([`compress`]), the port of part of the compressor
//!   of zstd itself (`lib/compress`): its two fastest strategies, which are the ones levels 1
//!   to 3 and the negative levels use, with the entropy coders that every strategy shares.
//!   The frames are the ones `ZSTD_compress` gives, byte for byte.
//! - [`zstd_decompress`]: frames to bytes ([`decompress`]), the port of the reference decoder
//!   that comes with zstd (`doc/educational_decoder`).
//!
//! The files of the compressor follow the original's: `bitstream`, `fse_compress`, `hist`
//! and `huf_compress` are the entropy coders, `zstd_compress_literals` and
//! `zstd_compress_sequences` the two sections of a block, `zstd_fast` and `zstd_double_fast`
//! the two strategies, `zstd_presplit` what decides where a block ends early, `clevels` the
//! table of levels, and `zstd_compress` the frame and its blocks.

mod bitstream;
mod clevels;
mod common;
#[doc(hidden)]
pub mod compare_hooks;
mod fse_compress;
mod hist;
mod huf_compress;
pub mod zstd_compress;
mod zstd_compress_literals;
mod zstd_compress_sequences;
pub mod zstd_decompress;
mod zstd_double_fast;
mod zstd_fast;
mod zstd_presplit;

pub use zstd_compress::{compress, compress_bound, level_is_ported};

pub use zstd_decompress::{decompress, decompress_with_dict, get_decompressed_size, Dictionary, ZstdError};
