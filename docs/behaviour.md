# Behaviour

[Overview](../README.md) · [Examples](examples.md) · [Guide](guide.md) · **Behaviour** · [Verification](verification.md) · [Performance](performance.md) · [Credits](credits.md)

---

- [Behavioural notes](#behavioural-notes)
- [Limitations](#limitations)

## Behavioural notes

### Block encoding

- **`bc7e` may decode opaque blocks with an alpha of 254.** Mode 6 carries alpha, and for a
  block without transparency the encoder disregards alpha when choosing the endpoints' least
  significant bits. Some or all pixels of such a block may then decode as 254 rather than
  255, exactly as with the original. A shader that ignores alpha on opaque textures is
  unaffected; to exclude the case entirely, disable mode 6 for opaque blocks
  (`opaque_settings.use_mode[6] = false`).

- **The ISPC ports match the strict builds of their originals.** `bc7e.ispc` and Intel's
  `kernel.ispc` are conventionally compiled with `--opt=fast-math`, which permits the
  compiler to reorder arithmetic. Such builds encode roughly 10% of blocks differently
  from strict builds, in the final bits of a decision rather than in kind. rustbcx
  reproduces the strict builds.

- **BC6H has two encoders.** `compress_hdr` uses Intel's, which is fast and encodes the
  unsigned variant only, clamping negative values to zero. `compress_hdr_signed` uses
  DirectXTex's, which also encodes the signed variant and is considerably slower. Both
  variants can be decoded.

- **BC6H encoding is weaker on very bright blocks.** The original stores each pixel's squared
  error as an integer, which very bright values overflow, so its choices among such blocks
  fall short of what its search would otherwise find. This is preserved, since the output
  depends on it.

- **Quirks of the originals are preserved**, because correcting them would alter the output.
  In `bc7e`, mode 7's table of optimal endpoints for a single colour carries the result of
  one p-bit pair over to the next. In the image encoder, the rate-distortion pass compares
  BC4 against the image's red channel, and BC5 against red and green, irrespective of the
  channels actually encoded.

- **`bc7enc` panics when its mode mask leaves a block with no permitted mode**, where the
  C++ asserts.

- **The originals' globals are values.** The tables that the C++ and ISPC code populate at
  start-up are owned by `Rgbcx::new`, `Bc7Enc::new` and `Bc7e::new`. No initialisation call
  is required, and each encoder can be shared between threads. The tables are built on
  first use and copied thereafter, so constructing an encoder per texture costs almost
  nothing.

- **Mip levels are box-filtered.** Each level is the plain average of two-by-two pixels of
  the stored values (of floats, for HDR). For another filter, or for averaging sRGB images
  in linear light, generate the levels yourself and encode each with `RdoBcEncoder`.

### Image files

- **`.png`, `.bmp` and `.jpg` are decoded exactly as stb_image decodes them.** JPEG decoders
  differ slightly in the pixels they produce; these are stb_image's. A file it rejects is
  rejected here, with the reason in the `LoadError`, and a damaged file it accepts is
  accepted here. PNG checksums are not verified, as in the original. `Image` holds 8 bits
  per channel, so a 16-bit `.png` is reduced to 8 on loading;
  `stb_image::load_16_from_memory` retains all 16.

- **`.jpg` is written as stb_image_write writes it**: baseline, with chroma at half
  resolution in each dimension up to quality 90 and at full resolution above it.

- **`.exr` is read and written as tinyexr does, with two exceptions.**
  1. *Uncompressible blocks.* The format specifies that a block which compression fails to
     shrink is stored as raw pixels. tinyexr observes this for RLE, ZIP and PIZ but not for
     PXR24 and B44. Other readers consequently misread some of its files (every B44 file
     without a half-float channel, and blocks of only a few pixels), and its own reader
     misreads such blocks from other writers and, occasionally, a PXR24 block of its own.
     rustbcx follows the format, as ffmpeg's reader confirms. The functions suffixed
     `_as_tinyexr` reproduce tinyexr's behaviour exactly, should you need its bytes.
  2. *Malformed files.* Four kinds of damaged file are rejected where tinyexr aborts or
     reads beyond the end of the file: a tile size of 0; a chunk count far exceeding what
     the file can hold; a compressed tile positioned beyond the image's edge; and a final
     uncompressed block shorter than its scanlines.

  A damaged header can still request a very large image, as with any image format. Inspect
  the header's data window first when files come from untrusted sources.

- **`HdrImage` stores red, green and blue as half floats.** `HdrImage::load` therefore
  narrows a file's full floats to halves and discards alpha and any other channels; `save`
  writes halves with lossless ZIP compression. For full floats, alpha, additional channels,
  tiles or another compression method, use the `tinyexr` module directly.

- **PIZ output matches tinyexr built with GNU's C++ standard library.** The lengths of PIZ's
  Huffman codes depend on how the C++ library orders a heap, so tinyexr built against
  another library may write different bytes for the same image. Each reads the other's
  output.

## Limitations

- **No hand-written SIMD by default.** Stable Rust offers no SIMD without `unsafe`, and the
  crate contains none. Its throughput comes from using every core and from loops the
  compiler vectorises automatically. On a single core, the ISPC originals built for AVX2 are
  faster. With a nightly compiler, the `nightly-simd` feature implements the hottest loop of
  `bc7e` with `std::simd` (see [Performance](performance.md)).

- **Zstandard compression is limited to levels 1 to 3 and the negative levels.** zstd has
  nine match-finding strategies, one per level; the two fastest are ported, which is what
  those levels use. (Level 4 uses one of them for some source sizes;
  `zstd::level_is_ported` reports which.) Higher levels, up to 22, are refused until their
  strategies are ported; the crate does not substitute a weaker one. Frames of every level
  can be decompressed. Frames are written whole, without a dictionary or checksum.

- **No BasisLZ.** The third `.ktx2` supercompression scheme cannot carry these formats at
  all; it is specific to Basis Universal's ETC1S format.

- **Image formats are limited to `.png`, `.bmp` and `.jpg`.** The other formats stb_image
  reads (`.tga`, `.gif`, `.psd`, `.hdr`, `.pnm`) are not ported.

- **`.exr` support is limited to single-part files.** Multi-part files, deep images, and the
  DWAA, DWAB and ZFP compression methods are neither read nor written.

- **x86 float-to-integer semantics are built in.** Where an original converts a float that
  does not fit into an integer, the ports return what x86 returns, on every processor, so
  the output is identical everywhere. The originals themselves would differ on other
  architectures.
