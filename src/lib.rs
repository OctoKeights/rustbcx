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
//!
//! And around the ports, the crate's own: [`bc2`], the one block format of the family none of
//! the originals has; [`texture`], an image packed with its mip levels; and [`dds`] and
//! [`ktx`], which read and write it as `.dds`, `.ktx` and `.ktx2` files.
//!
//! The ports keep the originals' behaviour, so a block comes out as the same bytes from both
//! (for the two ISPC originals, `bc7e.ispc` and Intel's `kernel.ispc`, as built without
//! fast-math). The crate has no dependencies and no `unsafe`.

#![forbid(unsafe_code)]
// The code follows the originals' line for line, and these are its shapes.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::manual_range_contains)]

pub mod bc2;
pub mod bc6h;
pub mod bc7decomp;
pub mod bc7e;
pub mod bc7enc;
pub mod dds;
pub mod ert;
mod fmath;
pub mod image;
pub mod ktx;
mod parallel;
pub mod rdo_bc_encoder;
pub mod rgbcx;
mod rgbcx_tables;
pub mod texture;

/// (The README's examples, compiled and run with the tests.)
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
