//! Zstandard, for `.ktx2` files whose levels are supercompressed with it.
//!
//! - [`zstd_decompress`]: frames to bytes, the port of the reference decoder that comes with
//!   zstd (`doc/educational_decoder`).

pub mod zstd_decompress;

pub use zstd_decompress::{decompress, decompress_with_dict, get_decompressed_size, Dictionary, ZstdError};
