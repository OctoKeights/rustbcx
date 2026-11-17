# Changelog

## 0.6.0

- **Breaking: the 0.2 releases of the ported crates.** miniz_nrp, zstd_nrp, stb_image_nrp,
  stb_image_write_nrp and tinyexr_nrp were each rewritten with an idiomatic API, and
  `rustbcx::miniz`, `rustbcx::zstd`, `rustbcx::stb_image`, `rustbcx::stb_image_write` and
  `rustbcx::tinyexr` now give those. Each crate's changelog lists its changes; their output
  is unchanged. Reading and writing image files is 2% to 117% faster, and Zstandard
  supercompressed `.ktx2` files decompress 27% to 67% faster.

- **Breaking: no panics on bad input.** Functions that panicked on arguments they could not
  use now return an error:

  - `bc7enc::Bc7Enc::compress_block` returns a `Result<bool, CompressBlockError>` when the
    settings leave no mode that can pack the block, or a forced selector is out of range;
    `RdoBcEncoder::encode` then returns false.

  - `bc7e::Bc7e::compress_blocks` and `compress_blocks_on`, and
    `bc6h::compress_blocks_bc6h` and `compress_blocks_bc6h_on`, return a `LengthMismatch`
    when there is not a block for each set of sixteen pixels.

  - `Image::half_with`, `Image::half_volume`, `HdrImage::half_with` and
    `HdrImage::half_volume` return a `SizeMismatch` when the images are not of one size.

- **Breaking: typed settings.** `Supercompression::Zlib` and `Supercompression::Zstd` take a
  `miniz::Level` and a `zstd::Level`, and `Image::to_jpg` takes its quality as a `u8`.

- **Documentation.** Every public item is documented, and the crate warns on any that is
  not (`missing_docs`).

## 0.5.0

- **Breaking: the image codecs and supercompression are crates of their own.** The ports of
  miniz, zstd, stb_image, stb_image_write and tinyexr, with tinyexr's DWA (of OpenEXR), its
  ZFP (of the zfp library) and the libdeflate DWA uses, have moved out of rustbcx into crates
  of their own, each under its original's licence and with its own comparison and tests:
  [miniz_nrp](https://github.com/OctoKeights/miniz_nrp),
  [zstd_nrp](https://github.com/OctoKeights/zstd_nrp),
  [stb_image_nrp](https://github.com/OctoKeights/stb_image_nrp),
  [stb_image_write_nrp](https://github.com/OctoKeights/stb_image_write_nrp),
  [tinyexr_nrp](https://github.com/OctoKeights/tinyexr_nrp),
  [openexr_nrp](https://github.com/OctoKeights/openexr_nrp),
  [zfp_nrp](https://github.com/OctoKeights/zfp_nrp) and
  [libdeflate_nrp](https://github.com/OctoKeights/libdeflate_nrp) ("_nrp" for native Rust
  port). rustbcx depends on them and gives five of them again under their old names, so
  `rustbcx::miniz`, `rustbcx::zstd`, `rustbcx::stb_image`, `rustbcx::stb_image_write` and
  `rustbcx::tinyexr` are as they were. `rustbcx::dwa` and `rustbcx::libdeflate` are gone:
  use `openexr_nrp::dwa`, and libdeflate_nrp, whose functions are now libdeflate's own
  (`zlib_compress`, `zlib_decompress` and the rest; OpenEXR's wrappers of them are
  `openexr_nrp::compression`). The deprecated `zstd::level_is_ported` is gone too. rustbcx's
  own licence (Apache 2.0) now covers only its own ports, and the other licences' files went
  with their ports.

- **Breaking: needs Rust 1.100.** The crate now uses Rust's 2024 edition, and its minimum
  Rust (`rust-version` in `Cargo.toml`) is 1.100, the version it is developed and checked
  with (`rust-toolchain.toml`). On an older Rust, Cargo refuses to build it and says why.

## 0.4.0

- **Every image format stb_image reads.** `stb_image` now ports all of its readers: `.tga`,
  `.psd`, `.gif`, `.pgm`, `.ppm` and Radiance `.hdr`, alongside `.png`, `.bmp` and `.jpg`.
  `loadf_from_memory` gives floats, `load_gif_from_memory` every frame of a GIF with its delay,
  and `is_hdr_from_memory` says whether a file is HDR. `Image::load` reads them all, and
  `HdrImage::load` reads `.hdr` as well as `.exr`. Checked against stb_image byte for byte on
  860,000 made-up files (the `stb_image_more` dump).

- **Multi-part and deep `.exr` files.** `tinyexr` now reads and writes files of several
  parts (`parse_exr_multipart_header_from_memory`, `load_exr_multipart_image_from_memory`,
  `save_exr_multipart_image_to_memory`) and reads deep images, of scanlines and of tiles
  (`load_deep_exr_from_memory`). tinyexr's readers for these read past the end of a damaged
  file; the port makes those reads with zeros and with ones there, and gives tinyexr's answer
  where it is the same either way, refusing the file with `PAST_THE_END` otherwise. Checked
  against tinyexr on 120,000 files, damaged and not (the `tinyexr_parts` and `tinyexr_deep`
  dumps).

- **DWAA and DWAB `.exr` compression.** `tinyexr` now reads and writes OpenEXR's lossy
  compressions (`COMPRESSIONTYPE_DWAA`, `COMPRESSIONTYPE_DWAB`), which tinyexr itself does
  not have, at the level of loss a header's `dwaCompressionLevel` attribute sets
  (`dwa_compression_level_attribute`; 45 by default). The new `dwa` module is a port of
  OpenEXR 3.5's OpenEXRCore code for them, a block at a time, and gives the blocks and pixels
  OpenEXR gives on an x86-64 machine with AVX and F16C, on every machine. Checked against
  OpenEXRCore on 400,000 made-up blocks compressed and decompressed, and 400,000 damaged
  (the `dwa_write` and `dwa_read` dumps); files written by each are read by the other.
  The `_as_tinyexr` functions refuse them as tinyexr does, with the new
  `parse_exr_header_from_memory_as_tinyexr` and `parse_exr_multipart_header_from_memory_as_tinyexr`.

- **libdeflate.** The new `libdeflate` module is a port of the parts of libdeflate 1.26 that
  OpenEXR uses: zlib streams at its levels 0 and 2 to 9, and back, with OpenEXR's handling of
  the answer. Checked against libdeflate on 40,000 made-up sources (the `libdeflate` dump).

- **ZFP `.exr` compression.** `tinyexr` now reads and writes the ZFP compression tinyexr
  adds to the format (`COMPRESSIONTYPE_ZFP`), with a port of the zfp library's coder of
  blocks of floats: at a fixed rate, a precision or a tolerance, set by the attributes
  `ZfpSettings` makes. Checked against tinyexr built with `TINYEXR_USE_ZFP` and zfp 1.0.1 on
  100,000 made-up images, saved and read back, a third of them damaged (the
  `tinyexr_zfp_write` and `tinyexr_zfp_read` dumps).

- **Every Zstandard level.** `zstd::compress` now takes levels 4 to 22 as well, with all
  nine of zstd's strategies: greedy and lazy (with hash chains, rows of a hash table, or a
  binary tree), and the optimal ones; with the splitting of blocks the slower levels do, the
  Huffman tables of the best depth, the choice of each sequence table by its cost, and the
  long-distance matching level 22 does for sources over 64 MB. Checked against
  `ZSTD_compress` on 6,000 sources of up to 5 MB (the `zstd_compress_more` dump) and on
  sources of 64 to 80 MB (`zstd_compress_long`), every frame byte for byte.
  `zstd::level_is_ported` is deprecated: it is always true.

- **Breaking:** `LoadOptions` gains the four gamma and scale settings and is no longer `Eq`;
  `Info` gains `is_hdr`; `ImageType` gains a variant for each new format.
  `tinyexr::parse_exr_header_from_memory` and `parse_exr_multipart_header_from_memory` now
  accept DWAA and DWAB files, which they refused; their `_as_tinyexr` counterparts keep
  tinyexr's answer.

## 0.3.2

Documentation only: the README is now a concise overview, with the detail moved to the pages
under `docs/`. No change to the code's behaviour.

## 0.3.0

Every block format of the family, every shape of texture, and the files around them: the
crate now reads the pictures it packs and compresses the files it writes.

- **BC2.** `bc2` packs and unpacks BC2 (DXT3) blocks: the colours by `rgbcx` as BC3's are, the
  alphas at four bits a pixel. The image encoder takes it as a format (`BlockFormat::Bc2`),
  with the rate-distortion pass, and `.dds`, `.ktx` and `.ktx2` files hold it. None of the
  originals has BC2, so this is the crate's own; the comparison checks it against bc7enc_rdo's
  rgbcx and bcdec (the `bc2` dump).
- **Cube maps, texture arrays and volume textures.** A `Texture` can be any of them (its new
  `depth`, `layers` and `cube`), and `.dds`, `.ktx` and `.ktx2` files are read and written
  with them. `texture::compress_many` and `compress_hdr_many` pack several images into one,
  with mip levels; a volume's levels halve its slices too (`Image::half_volume`).
  `Texture::image` and `unpack_image` give one image of a level.
- **Signed BC6H packing.** `bc6h_enc` is the port of bc6h_enc, DirectXTex's BC6H encoder as
  one file: `encode_bc6hs` packs signed blocks and `encode_bc6hu` unsigned ones, thoroughly
  and slowly. `texture::compress_hdr_signed` and `compress_hdr_many_signed` pack images with
  it, and `HdrImage::from_rgb_f32_signed` makes an image that keeps what is below zero.
- **zlib, and `.ktx2` with ZLIB supercompression.** `miniz` is the port of miniz's compressor
  and decompressor: `miniz::compress` and `uncompress` for zlib streams, and the compressor
  and decompressor themselves under `miniz::tdef` and `miniz::tinfl`. `ktx::write_ktx2_with`
  and `Texture::save_ktx2` write a `.ktx2` file with each level compressed
  (`ktx::Supercompression::Zlib`), and `read_ktx2` reads one.
- **`.png`, `.bmp` and `.jpg` files, read and written.** `Image::load` and `save` read and
  write them (`from_file_bytes`, `to_png`, `to_bmp` and `to_jpg` for the bytes), so an image
  no longer has to come in through another crate. Under them are two ports. `stb_image` is
  stb_image.h's PNG, BMP and JPEG readers (`load_from_memory`, `load_16_from_memory` for 16
  bits a channel, `info_from_memory`), with its zlib decoder: PNG of every depth and colour
  type, interlaced or not; BMP without run-length encoding; JPEG baseline and progressive.
  It gives the pixels the original gives and refuses what it refuses, with the same reason.
  `stb_image_write` is stb_image_write.h's three writers, `write_png`, `write_bmp` and
  `write_jpg`, each giving the bytes of the file, the same as the original writes;
  `zlib_compress` is the small zlib compressor its PNG writer uses.
- **`.ktx2` files with Zstandard supercompression are read.** `zstd::decompress` is the port
  of the reference decoder that comes with zstd (its `doc/educational_decoder`), with
  `decompress_with_dict` and `Dictionary::parse` for frames made with a dictionary.
  `read_ktx2` uses it for files whose levels are compressed with Zstandard.
- **`.ktx2` files are written with Zstandard supercompression**
  (`ktx::Supercompression::Zstd(level)`), and **`zstd::compress` makes Zstandard frames.** It
  is the port of part of zstd's own compressor, v1.5.7: the two fastest of its nine
  strategies, `ZSTD_fast` and `ZSTD_dfast`, with the block splitting, the Huffman and FSE
  coders and the framing that all strategies share. A frame is byte for byte the one
  `ZSTD_compress` gives for the same source and level. The two strategies are the ones
  levels 1 to 3 and the negative levels use for a source of any size, and level 4 for some
  sizes; `zstd::level_is_ported` says whether a level is taken, and the others are refused
  until their strategies are ported.
- **`.exr` files, read and written.** `HdrImage::load` and `save` read and write them
  (`from_file_bytes` and `to_exr` for the bytes; `load_signed` and `from_file_bytes_signed`
  keep what is below zero), so an HDR image no longer has to come in through another crate.
  Under them is `tinyexr`, the port of Syoyo Fujita's tinyexr (`tinyexr.h`, v3.4.0):
  `load_exr_from_memory` and `save_exr_to_memory` for floats of red, green, blue and alpha;
  `parse_exr_version_from_memory`, `parse_exr_header_from_memory`,
  `load_exr_image_from_memory` and `save_exr_image_to_memory` for a file's header and every
  channel as it is stored (halves, floats or 32-bit integers). Files of scanlines and of
  tiles, with mip and rip levels, uncompressed or compressed with RLE, ZIPS, ZIP, PIZ,
  PXR24, B44 or B44A. It writes the files the original writes, byte for byte, gives the
  values it gives and refuses what it refuses, with the same code and message; but PXR24 and
  B44 blocks that do not come out smaller than their pixels are stored and read as the
  format has them, which tinyexr does not do and other readers need (the functions ending in
  `_as_tinyexr` do as tinyexr does). Files of several parts and deep images are left out.
- **The encoders' tables are made once.** `Bc7e::new`, `Bc7Enc::new` and `Rgbcx::new` made
  their lookup tables on every call, which took 26 milliseconds for the three, and
  `texture::compress` paid that for every texture. They are now made the first time and
  copied afterwards, which takes 20 microseconds: the same blocks, and a good deal of time
  saved where many small textures are packed one after the other.
- **The `nightly-simd` feature**, for nightly compilers: `bc7e` weighs a set of endpoints for
  four pixels at a time with `std::simd`. The same blocks, about a tenth faster at the
  `basic` profile and above. The build without it is unchanged: the same loop written for
  the compiler to vectorise by itself was slower than the original's, and was not kept.
- Fixed: a `.ktx2` file of an sRGB texture in BC3 did not mark its alpha as linear in its data
  format descriptor, as the Khronos tools do and their validator requires.
- Changed: `Texture` has three more fields, so one written out by hand needs them;
  `Texture::new` makes a plain one. `Texture::level_bytes` is now the bytes of a whole level,
  and `image_bytes` those of one image in it (the same for a plain texture).

## 0.2.0

What 0.1.0 listed as its limitations, taken on:

- **The rate-distortion pass.** `ert` is the port of bc7enc_rdo's `ert.cpp`: it goes over
  packed blocks and makes them compress better afterwards, at a small cost in quality.
- **Whole images, on all cores.** `rdo_bc_encoder` is the port of bc7enc_rdo's
  `rdo_bc_encoder.cpp`: an image to BC1, BC3, BC4, BC5 or BC7 blocks, with the rate-distortion
  pass. It uses every core the machine has unless `RdoBcParams::threads` says otherwise, and
  the blocks do not depend on the number. `Bc7e::compress_blocks` shares its blocks out the
  same way (`compress_blocks_on` takes the number of threads).
- **BC6H.** `bc6h` unpacks both kinds (the port of bcdec's decoder) and packs the unsigned kind
  with five profiles (the port of the encoder of Intel's ISPC Texture Compressor, matched to a
  strict build of it as `bc7e` is).
- **Textures, mip levels and files.** `texture::compress` and `texture::compress_hdr` take an
  image to a `Texture` with every mip level; `Texture::save` and `Texture::load` write and read
  `.dds`, `.ktx` and `.ktx2` files (`dds`, `ktx`). These parts are the crate's own.
- **Speed.** `bc7e` is about 15% faster and the two BC7 encoders no longer call a software
  `floor`. `examples/bench.rs` times every encoder on one thread and on all.
- The blocks of 0.1.0's encoders and decoders are unchanged.
- The comparison (`compare/compare.sh`) covers all of the above; it takes the folders of bcdec
  and the ISPC Texture Compressor in `$BCDEC` and `$ISPCTC`.
- The crate still has no dependencies and no `unsafe`. SIMD by hand would need `unsafe` in
  stable Rust, so it is left out; the README says where that leaves the speed.

## 0.1.0

- First release: the port of bc7enc_rdo's `rgbcx` (BC1, BC3, BC4 and BC5, both ways),
  `bc7decomp` (BC7 to pixels) and `bc7enc` (pixels to BC7 in modes 1, 5, 6 and 7), and of
  `bc7e.ispc` (pixels to BC7 in all eight modes, with its seven profiles).
- Every block is the one the originals produce; for `bc7e`, the original built without
  fast-math. `compare/compare.sh` checks it against both.
