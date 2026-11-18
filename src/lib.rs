//! GPU texture compression, BC1 to BC7: ports to Rust, function for function, of Richard
//! Geldreich's [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo), of the encoder that
//! comes with it, Binomial's `bc7e.ispc`, and for BC6H of bcdec and Intel's ISPC Texture
//! Compressor; and around them, textures with mip levels and their files.
//!
//! - [`bc7decomp`]: BC7 blocks to pixels (`bc7decomp.cpp`).
//! - [`bc7enc`]: BC7 blocks from pixels, the small and quick encoder: modes 1 and 6 for opaque
//!   blocks, modes 5, 6 and 7 for blocks with alpha (`bc7enc.cpp`).
//! - [`bc7e`]: BC7 blocks from pixels, the thorough encoder: all eight modes, seven profiles
//!   from fastest to best (`bc7e.ispc`).
//! - [`rgbcx`]: BC1, BC3, BC4 and BC5 blocks from pixels and back (`rgbcx.cpp`).
//! - [`ert`]: the rate-distortion pass over packed blocks of any of these formats, which makes
//!   them compress better afterwards at a small cost in quality (`ert.cpp`).
//! - [`rdo_bc_encoder`]: whole images to blocks of any of these formats, on all the machine's
//!   cores, with the rate-distortion pass (`rdo_bc_encoder.cpp`). [`image`] has the image it
//!   takes.
//! - [`bc6h`]: BC6H, the format for HDR colour: blocks to pixels (`bcdec.h`, from
//!   Sergii Kudlai's [bcdec](https://github.com/iOrange/bcdec)) and pixels to blocks
//!   (`kernel.ispc`, from Intel's
//!   [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor)).
//! - [`bc6h_enc`]: BC6H again, pixels to blocks the thorough way, and to signed blocks as well
//!   as unsigned (`bc6h_enc.h`, Alexander Samusev's [bc6h_enc](https://github.com/0xc0de/bc6h_enc),
//!   which is the encoder of Microsoft's DirectXTex).
//!
//! The image files and the supercompression of `.ktx2` are ports too, each a crate of its own
//! under its original's licence, which this crate uses and gives again under their old names:
//!
//! - [`miniz`]: zlib and deflate, both ways
//!   ([miniz_nrp](https://github.com/OctoKeights/miniz_nrp), of Rich Geldreich's
//!   [miniz](https://github.com/richgel999/miniz)).
//! - [`zstd`]: Zstandard, both ways, at every level of zstd
//!   ([zstd_nrp](https://github.com/OctoKeights/zstd_nrp), of
//!   [zstd](https://github.com/facebook/zstd)).
//! - [`stb_image`] and [`stb_image_write`]: `.png`, `.bmp`, `.jpg`, `.tga`, `.psd`, `.gif`,
//!   `.pgm`, `.ppm` and `.hdr` files to images, and `.png`, `.bmp` and `.jpg` back
//!   ([stb_image_nrp](https://github.com/OctoKeights/stb_image_nrp) and
//!   [stb_image_write_nrp](https://github.com/OctoKeights/stb_image_write_nrp), of Sean
//!   Barrett's [stb](https://github.com/nothings/stb)).
//! - [`tinyexr`]: `.exr` files to halves, floats or integers and back
//!   ([tinyexr_nrp](https://github.com/OctoKeights/tinyexr_nrp), of Syoyo Fujita's
//!   [tinyexr](https://github.com/syoyo/tinyexr)), with OpenEXR's DWAA and DWAB
//!   ([openexr_nrp](https://github.com/OctoKeights/openexr_nrp)) and tinyexr's ZFP
//!   ([zfp_nrp](https://github.com/OctoKeights/zfp_nrp)).
//!
//! And around the ports, the crate's own: [`bc2`], the one block format of the family none of
//! the originals has; [`image`], the images the encoders take, read from picture files and
//! written to them; [`texture`], an image packed with its mip levels, or several as a texture
//! array, a cube map or a volume texture; and [`dds`] and [`ktx`], which read and write it as
//! `.dds`, `.ktx` and `.ktx2` files.
//!
//! The ports keep the originals' behaviour, so a block comes out as the same bytes from both
//! (for the two ISPC originals, `bc7e.ispc` and Intel's `kernel.ispc`, as built without
//! fast-math). The crate has no `unsafe`, and no dependencies but those ports.
//!
//! # Cargo features
//!
//! - `nightly-simd`: the hottest loop of [`bc7e`] written with `std::simd`, which needs a
//!   nightly compiler. The blocks are the same with and without it.

#![forbid(unsafe_code)]
#![cfg_attr(feature = "nightly-simd", feature(portable_simd))]
#![warn(missing_docs)]

pub mod bc2;
pub mod bc6h;
pub mod bc6h_enc;
mod bc6h_enc_tables;
pub mod bc7decomp;
pub mod bc7e;
pub mod bc7enc;
pub mod dds;
pub mod ert;
mod fmath;
pub mod image;
pub mod ktx;
mod lanes;
mod parallel;
pub mod rdo_bc_encoder;
pub mod rgbcx;
mod rgbcx_tables;
pub mod texture;

/// Blocks and the sets of sixteen pixels to pack into them, which are not as many as each
/// other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LengthMismatch {
    /// How many blocks there are.
    pub blocks: usize,
    /// How many sets of sixteen pixels there are.
    pub pixels: usize,
}

impl LengthMismatch {
    /// `Ok` when there is a block for each set of pixels.
    pub(crate) fn check(blocks: usize, pixels: usize) -> Result<(), LengthMismatch> {
        if blocks == pixels {
            Ok(())
        } else {
            Err(LengthMismatch { blocks, pixels })
        }
    }
}

impl std::fmt::Display for LengthMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} blocks for {} sets of sixteen pixels",
            self.blocks, self.pixels
        )
    }
}

impl std::error::Error for LengthMismatch {}

// The ports that are crates of their own, under the names they had here.
pub use miniz_nrp as miniz;
pub use stb_image_nrp as stb_image;
pub use stb_image_write_nrp as stb_image_write;
pub use tinyexr_nrp as tinyexr;
pub use zstd_nrp as zstd;

/// (The README's examples, compiled and run with the tests.)
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// (And those of `docs/examples.md`.)
#[cfg(doctest)]
#[doc = include_str!("../docs/examples.md")]
struct ExamplesDoctests;

/// The two halves of a 16-byte block: for BC2 and BC3 its alpha and its colour, for BC5 its two
/// channels.
#[inline]
pub(crate) fn halves(block: &[u8; 16]) -> (&[u8; 8], &[u8; 8]) {
    let (first, second) = block.split_at(8);
    (
        first.try_into().expect("8 bytes"),
        second.try_into().expect("8 bytes"),
    )
}

/// [`halves`], to write.
#[inline]
pub(crate) fn halves_mut(block: &mut [u8; 16]) -> (&mut [u8; 8], &mut [u8; 8]) {
    let (first, second) = block.split_at_mut(8);
    (
        first.try_into().expect("8 bytes"),
        second.try_into().expect("8 bytes"),
    )
}
