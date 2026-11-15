# Changelog

## Not yet released

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
- **`.exr` files are read.** `tinyexr` is the port of the reader in Syoyo Fujita's tinyexr
  (`tinyexr.h`, v3.4.0): `parse_exr_version_from_memory` and `parse_exr_header_from_memory`
  for a file's header, `load_exr_image_from_memory` for its channels as they are stored
  (halves, floats or 32-bit integers; halves as floats if asked), and `load_exr_from_memory`
  for red, green, blue and alpha as floats. Files of scanlines and of tiles, with mip and rip
  levels, uncompressed or compressed with RLE, ZIPS, ZIP, PIZ, PXR24, B44 or B44A. It gives
  the values the original gives and refuses what it refuses, with the same code and message.
  Files of several parts and deep images are not read.
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
