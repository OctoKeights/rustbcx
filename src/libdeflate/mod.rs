//! zlib streams, both ways, as OpenEXR makes and reads them for its DWAA and DWAB
//! compressions: a port of the parts of Eric Biggers's
//! [libdeflate](https://github.com/ebiggers/libdeflate) v1.26, the version OpenEXR 3.5 comes
//! with, that it uses.
//!
//! - [`compress_buffer`]: bytes to a zlib stream at one of libdeflate's levels
//!   (`deflate_compress.c`, `zlib_compress.c`), as `exr_compress_buffer` asks for it.
//! - [`uncompress_buffer`]: a zlib stream back to bytes (`deflate_decompress.c`,
//!   `zlib_decompress.c`), with what OpenEXR's `exr_uncompress_buffer` makes of its answer.
//!
//! libdeflate's output is its own: another DEFLATE compressor (miniz's, say) makes other
//! streams of the same bytes, which is why it is ported apart from [`crate::miniz`].

mod compress;
mod decompress;

pub use compress::{MAX_LEVEL, compress_bound, compress_buffer};
pub(crate) use decompress::adler32;
pub use decompress::uncompress_buffer;

/// Why a stream was not decompressed, as OpenEXR tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The stream is not one, or does not end where its input does
    /// (`EXR_ERR_CORRUPT_CHUNK`).
    Corrupt,
    /// It makes more bytes than there is room for (`EXR_ERR_OUT_OF_MEMORY`).
    OutOfSpace,
}

/// A decompression's answer.
pub type Result<T> = core::result::Result<T, Error>;
