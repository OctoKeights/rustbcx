# Guide

[Overview](../README.md) · [Examples](examples.md) · **Guide** · [Behaviour](behaviour.md) · [Verification](verification.md) · [Performance](performance.md) · [Credits](credits.md)

---

- [Module reference](#module-reference)
- [Selecting a BC7 encoder](#selecting-a-bc7-encoder)
- [Rate-distortion optimisation](#rate-distortion-optimisation)
- [Threading](#threading)
- [Suitability](#suitability)

## Module reference

### Ports

Each port is verified byte for byte against its original (see
[Verification](verification.md)).

| Module | Ported from | Purpose |
|---|---|---|
| `rgbcx` | bc7enc_rdo, `rgbcx.cpp` | BC1, BC3, BC4 and BC5 encoding and decoding |
| `bc7decomp` | bc7enc_rdo, `bc7decomp.cpp` | BC7 decoding, all modes |
| `bc7enc` | bc7enc_rdo, `bc7enc.cpp` | Compact, fast BC7 encoder: modes 1 and 6 for opaque blocks; modes 5, 6 and 7 for blocks with alpha |
| `bc7e` | Binomial's `bc7e.ispc` | Exhaustive BC7 encoder: all eight modes, seven quality profiles |
| `ert` | bc7enc_rdo, `ert.cpp` | Rate-distortion pass over encoded blocks of any of these formats |
| `rdo_bc_encoder` | bc7enc_rdo, `rdo_bc_encoder.cpp` | Whole-image encoding to BC1, BC3, BC4, BC5 or BC7 with the rate-distortion pass (BC2 is a rustbcx addition) |
| `bc6h` (decoding) | [bcdec](https://github.com/iOrange/bcdec), `bcdec.h` | BC6H decoding, signed and unsigned, to half or full floats |
| `bc6h` (encoding) | Intel's [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor), `kernel.ispc` | Unsigned BC6H encoding, five profiles |
| `bc6h_enc` | [bc6h_enc](https://github.com/0xc0de/bc6h_enc), the single-file form of DirectXTex's BC6H encoder | Signed and unsigned BC6H encoding; exhaustive and slow |
| `miniz` | Rich Geldreich's [miniz](https://github.com/richgel999/miniz): compressor, decompressor and checksums | zlib and deflate in both directions, for ZLIB-supercompressed `.ktx2` files |
| `zstd` | [zstd](https://github.com/facebook/zstd): the two fastest strategies and the entropy coders of `lib/compress`, and the reference decoder in `doc/educational_decoder` | Zstandard in both directions, for Zstandard-supercompressed `.ktx2` files. Compresses at levels 1 to 3 and the negative levels; decompresses frames of any level |
| `stb_image` | Sean Barrett's [stb](https://github.com/nothings/stb), `stb_image.h`: the PNG, BMP and JPEG readers and the zlib decoder | Decodes `.png`, `.bmp` and `.jpg` at 8 or 16 bits per channel |
| `stb_image_write` | stb, `stb_image_write.h`: the PNG, BMP and JPEG writers | Encodes 8-bit pixels to `.png`, `.bmp` and `.jpg` |
| `tinyexr` | Syoyo Fujita's [tinyexr](https://github.com/syoyo/tinyexr), `tinyexr.h`: the single-part reader and writer | Reads and writes `.exr` as half, float or integer channels: scanlines or tiles, mip and rip levels, uncompressed or RLE, ZIP, PIZ, PXR24 or B44 |

### Original code

These modules have no upstream counterpart.

| Module | Purpose |
|---|---|
| `bc2` | BC2 (DXT3), which none of the originals provides. Colour is encoded by `rgbcx` as for BC3; alpha is stored at four bits per pixel. The decoder is verified against bcdec |
| `image` | `Image` (8-bit RGBA) and `HdrImage` (half-float RGB), with mip-level generation. `Image::load` and `save` handle `.png`, `.bmp` and `.jpg`; `HdrImage::load` and `save` handle `.exr` |
| `texture` | `Texture`: a format, dimensions and the blocks of every mip level, for a single image, a texture array, a cube map or a volume texture. `compress` and `compress_hdr` build one from an image; `compress_many` and `compress_hdr_many` from several; `load` and `save` handle files |
| `dds` | `.dds` reading and writing, with either header. For a single level, the writer produces the same bytes as bc7enc_rdo's `save_dds` |
| `ktx` | `.ktx` (KTX 1) and `.ktx2` (KTX 2) reading and writing; `.ktx2` levels may be stored raw or supercompressed with zlib or Zstandard |

## Selecting a BC7 encoder

- **`bc7e`** where quality is the priority. With access to every mode, it handles blocks that
  the two or three modes of `bc7enc` represent poorly. Its profiles are those of the
  original, fastest first: `ultrafast`, `veryfast`, `fast`, `basic`, `slow`, `veryslow`,
  `slowest`. `RdoBcParams` uses it unless `use_bc7e` is disabled, mirroring bc7enc_rdo when
  built with it.
- **`bc7enc`** where output must match bc7enc_rdo built without ISPC, or where a compact
  encoder with few settings is preferred.

The two encoders produce different blocks from the same pixels; each matches its own
original.

## Rate-distortion optimisation

Encoded textures are usually compressed again for storage or distribution (zip, zstd, LZMA).
The bc7enc_rdo pass revisits the encoded blocks and, where the cost in quality is low,
reuses byte sequences from preceding blocks, which is precisely the redundancy such
compressors exploit.

Set `RdoBcParams::rdo_lambda` above 0 to enable it. Values of roughly 0.5 to 2 give a modest
trade-off; higher values yield smaller files and more visible error. Smooth regions, where
changes are most conspicuous, are protected automatically.

One setting affects the output and is easily overlooked. An image of 4,096 blocks or more is
processed in `rdo_max_threads` independent strips, and a strip cannot reference the one
before it. The default is 128 on every machine, so a given image always produces the same
texture. (The bc7enc_rdo command line instead derives this value from the core count.)

## Threading

Whole-image encoding (`texture::compress`, `texture::compress_hdr`, `RdoBcEncoder`) and batch
block encoding (`Bc7e::compress_blocks`, `bc6h::compress_blocks_bc6h`) use every available
core.

To restrict them, set `RdoBcParams::threads`, pass a thread count to `compress_hdr`, or call
the `_on` variants of the batch functions. A value of 1 keeps the work on the calling
thread. The output is identical for any thread count.

Threads come from the standard library and exist only for the duration of the call.

## Suitability

Rust already offers several routes to these formats, including bindings to the C++ and ISPC
encoders themselves, such as [rgbcx-sys](https://crates.io/crates/rgbcx-sys) and
[intel_tex_2](https://crates.io/crates/intel_tex_2). If building C++ or ISPC as part of your
project is acceptable and per-core throughput is the priority, one of those may serve you
better: the ISPC encoders process several blocks at once with SIMD, whereas rustbcx
processes one block at a time on each core.

rustbcx addresses a narrower requirement:

- You are migrating a C++ project that uses bc7enc_rdo to Rust, and its textures must not
  change.
- You require identical blocks on every machine and from every build, which the customary
  fast-math builds of the ISPC encoders do not guarantee.
- You want a self-contained texture compressor with no dependencies and no `unsafe`.

The crate was written for Noolith, a project whose C++ code compressed its textures with
bc7enc_rdo and whose Rust rewrite had to produce exactly the same blocks.
