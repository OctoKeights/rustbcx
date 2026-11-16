# Verification

[Overview](../README.md) · [Examples](examples.md) · [Guide](guide.md) · [Behaviour](behaviour.md) · **Verification** · [Performance](performance.md) · [Credits](credits.md)

---

- [Differential testing against the originals](#differential-testing-against-the-originals)
- [Coverage](#coverage)
- [Results](#results)
- [The ISPC originals](#the-ispc-originals)
- [The JPEG decoder](#the-jpeg-decoder)
- [Golden tests](#golden-tests)
- [Container and file interoperability](#container-and-file-interoperability)

## Differential testing against the originals

The repository includes a harness that compares each port with its original directly.
`compare/compare.sh` builds the originals, runs synthetic blocks, images and files through
both implementations, and confirms that the results are identical byte for byte:

```text
ISPC=<the ispc compiler> BCDEC=<checkout of bcdec> ISPCTC=<checkout of ISPCTextureCompressor> \
    BC6HENC=<checkout of bc6h_enc> MINIZ=<checkout of miniz> STB=<checkout of stb> \
    ZSTD=<checkout of zstd> TINYEXR=<checkout of tinyexr> ZFP=<checkout of zfp 1.0.1> \
    OPENEXR=<checkout of OpenEXR 3.5> OPENEXR_BUILD=<its CMake build folder> \
    compare/compare.sh <checkout of bc7enc_rdo> [blocks] [seeds] [dumps]
```

It requires a C and a C++ compiler, the [ISPC compiler](https://github.com/ispc/ispc/releases)
for the two ISPC originals, and the checkouts named above. Anything it cannot build is
reported as not compared, and the run fails.

## Coverage

### Block codecs

| Dump | Reference | Scope |
|---|---|---|
| `bc7decomp` | bc7enc_rdo | BC7 blocks of every mode, and invalid blocks, decoded |
| `unpack` | bc7enc_rdo | BC1, BC3, BC4 and BC5 blocks decoded in every variant |
| `bc1`, `bc345` | bc7enc_rdo | Pixels encoded at every level and with every flag, for each GPU interpolation mode |
| `bc7enc` | bc7enc_rdo | Pixels encoded as BC7 with every setting varied |
| `bc7block` | bc7enc_rdo | BC7 blocks of every mode written from synthetic endpoints and selectors |
| `ert` | bc7enc_rdo | Runs of near-duplicate blocks in five formats through the rate-distortion pass, with varied settings |
| `rdo` | bc7enc_rdo | Whole images of every shape through the image encoder in each format, with and without the pass, and the resulting `.dds` file |
| `rdo_bc7e` | bc7enc_rdo with `bc7e.ispc` | As `rdo`, with BC7 encoded by bc7e for half the images |
| `bc7e_presets`, `bc7e_settings` | `bc7e.ispc`, strict build | The seven profiles with both error metrics; then about 24,000 synthetic settings |
| `bc2` | bc7enc_rdo's rgbcx and bcdec | BC2 blocks decoded (alpha against bcdec, colour against rgbcx's BC1), and pixels encoded at every level and with every flag |
| `bc6h_unpack` | bcdec | BC6H blocks of every mode, and invalid blocks, decoded as signed and unsigned |
| `bc6h_pack` | Intel's `kernel.ispc`, strict build | HDR pixels encoded with each of the five profiles |
| `bc6h_enc` | bc6h_enc, built with `BC6H_SSE_INTRINSICS` | HDR pixels of both signs encoded as signed and unsigned BC6H |

### Compression and image codecs

| Dump | Reference | Scope |
|---|---|---|
| `miniz` | miniz | Synthetic data compressed at every level, with every strategy and with randomised flags, in one call and incrementally with intervening flushes. Each stream is decompressed whole, into insufficient space, corrupted, truncated, and with trailing bytes |
| `stb_image_write` | stb, `stb_image_write.h` | Synthetic images of one to four channels written as `.png` (varied compression levels and filters), `.bmp` and `.jpg` (varied qualities), some vertically flipped and some with a stride exceeding the row width; and synthetic data through its zlib compressor |
| `stb_image` | stb, `stb_image.h`, built with `STBI_NO_SIMD` | Synthetic files queried for dimensions and decoded at 8 and 16 bits per channel with every requested channel count: images written by stb_image_write, and files assembled from every header variant the readers accept (PNG of every depth and colour type, with palettes, transparency keys and interlacing; BMP of every header size and depth, with palettes and bit masks; baseline and progressive JPEG of one to four components with every sampling arrangement and restart markers). One third are corrupted first, and for those the rejection reason is compared. Synthetic zlib streams, intact and corrupted, are run through its zlib decoder |
| `stb_image_more` | stb, `stb_image.h`, built with `STBI_NO_SIMD` | Synthetic `.tga` files of every kind (grey, colour and palette, 8 to 32 bits, raw and run-length encoded, either orientation); `.psd` files of none to five channels at 8 and 16 bits, raw and run-length encoded; `.gif` files of one to three frames with global and local colour tables, interlacing, transparency and every disposal method, LZW-encoded by the harness; binary and text `.pgm` and `.ppm` files with every kind of maximum value; and Radiance `.hdr` files, flat and run-length encoded. One third are corrupted first. Each is queried for dimensions and HDR-ness, and decoded at 8 bits, 16 bits and as floats with varied gamma and scale; GIFs are also decoded as animations, every frame with its delay |
| `zstd_decode` | zstd's reference decoder | Frames produced by the zstd library from synthetic data: every level from the fastest to 22, with and without checksums and content sizes, with small windows, long-distance matching, targeted block sizes and flushes, and with raw-content and trained dictionaries. One third are corrupted first, and for those the error message is compared |
| `zstd_compress` | zstd's library, `ZSTD_compress` | Synthetic data of thirteen kinds, from empty to five megabytes (noise, text, runs, repeats, concatenated mixtures, short repeating patterns), compressed at levels 1 to 3, at the default, at level 4 for some sizes, and at negative levels down to the minimum. Each frame is also decoded |
| `zstd_compress_more` | zstd's library, `ZSTD_compress` | The same data at levels 4 to 22 (and occasionally higher, which is 22): every strategy from greedy to the second ultra one, with the hash-chain, row and binary-tree match finders, the optimal parser and the splitting of blocks |
| `zstd_compress_long` | zstd's library, `ZSTD_compress` | 64 to 80 megabytes at level 22, which uses long-distance matching for that much: pieces of the other kinds and copies of earlier data from up to 70 megabytes back, slightly altered |
| `zstd_entropy` | zstd's library, internal functions | The entropy coders one function at a time, covering cases a whole frame seldom or never exercises: synthetic counts normalised, written into possibly insufficient space and encoded (FSE); synthetic bytes Huffman-coded with one or four streams, with and without the previous call's table, into possibly insufficient space; and a block's literals section |
| `tinyexr_write` | tinyexr, `tinyexr.h`, built without threads | Synthetic images saved as `.exr`: scanlines and tiles with mip and rip levels; half, float and integer channels, preserved or converted; each of the eight compression methods; custom attributes; and one-, three- and four-channel float images saved through the simplified interface. The bytes of each file are compared and, where tinyexr declines to save an image, its message |
| `tinyexr_read` | tinyexr, `tinyexr.h`, built without threads | The files tinyexr saved, occasionally with the data window shifted or the line order reversed. One third are corrupted first: bits flipped, the end truncated, and header, offset-table and block values randomised. For each, the version, header, every channel or tile (as stored, or halves as floats) and the image as RGBA floats are compared and, for a rejected file, the code and message |
| `tinyexr_parts` | tinyexr, `tinyexr.h`, built without threads | Files of one to four parts, each part an image as `tinyexr_write` makes them, named (occasionally unnamed or duplicating the previous name), saved; the bytes or the message are compared. One third are corrupted first: bits flipped, the end truncated, header bytes, offsets and part numbers randomised. Each is read by tinyexr in a separate process twice, with zeros and with ones beyond its end: the version, every header, and every part's channels or tiles. Where the two readings differ, or tinyexr crashes, the port must reject the file |
| `tinyexr_deep` | tinyexr, `tinyexr.h`, built without threads | Synthetic deep files (tinyexr writes none) of scanlines and of single-level tiles, with one to three channels of every type, every compression method and up to forty samples a pixel. One third are corrupted. Each is read by tinyexr in a separate process twice, with zeros and with ones beyond its end and inaccessible memory before it and further on; the result, error message, sample counts and samples are compared as for `tinyexr_parts` |
| `tinyexr_zfp_write` | tinyexr, `tinyexr.h`, built with `TINYEXR_USE_ZFP` against zfp 1.0.1 | Synthetic images for ZFP compression, saved: float, half and integer channels stored as floats (occasionally as halves, which tinyexr refuses), most of them of whole four by four blocks, scanlines and tiles (occasionally with mip and rip levels); the settings at a fixed rate, a precision or a tolerance, among them zero, negative, infinite and not-a-number values, and occasionally missing or malformed. The bytes or the message are compared |
| `tinyexr_zfp_read` | the same | The files tinyexr saved of such images, one third corrupted as for `tinyexr_read` and some with their ZFP settings altered, read and compared as for `tinyexr_read` |
| `libdeflate` | libdeflate 1.26, as OpenEXR 3.5 bundles it | Synthetic data of eight kinds and many sizes, occasionally several pieces together, compressed into zlib streams at levels 0 and 2 to 9 as OpenEXR asks for them. Each stream is decompressed whole, into insufficient and into excess space, corrupted, truncated and with trailing bytes; and made-up bytes after a zlib header |
| `dwa_write` | OpenEXRCore of OpenEXR 3.5, on x86-64 with AVX and F16C | Synthetic blocks of one to six channels (halves, floats and integers; named as DWA's rules expect and otherwise; occasionally linear), up to 400 pixels wide and up to 32 or 256 lines, of eight kinds of values, among them every bit pattern; each written by OpenEXRCore as the only block of a file of scanlines or of one tile, compressed with DWAA or DWAB at a synthetic level, and decompressed. The bytes of each block and the decompressed lines are compared |
| `dwa_read` | the same | The same blocks, corrupted in one of eleven ways (bytes flipped, truncated, a counter or the compression of the AC coefficients altered, the channel rules altered or replaced by synthetic ones, an older version, trailing bytes, synthetic counters and data) and decompressed; the result code and the lines are compared |

The tinyexr dumps exercise the functions that reproduce tinyexr exactly. Those without
the `_as_tinyexr` suffix differ in one respect, which `compare/files.sh` verifies with an
independent reader (see [Container and file interoperability](#container-and-file-interoperability)
and [Behaviour](behaviour.md#image-files)).

## Results

The comparisons were run on Linux (x86-64, with AVX and F16C), with the C++ built by gcc and
the ISPC by ISPC 1.32.0. All results were identical.

| Comparison | Volume |
|---|---|
| `bc7enc` | 3 million blocks with every setting varied, and 3 million written in all eight modes; the C++ built by gcc and clang at several optimisation levels; repeated with Rust's overflow checks enabled |
| `bc7e` | 1.68 million blocks through the seven profiles; 780,000 through about 24,000 synthetic settings |
| `ert` | 160,000 blocks in runs of five formats |
| `rdo`, `rdo_bc7e` | Over 100 seeds of images of 2,500 to 10,000 blocks each; the Rust run on all cores, on one thread and on three |
| `bc2` | 600,000 blocks decoded each way and as many encoded twice |
| `bc6h_unpack` | 1.5 million blocks, each as signed and as unsigned |
| `bc6h_pack` | 240,000 blocks through each of the five profiles |
| `bc6h_enc` | 60,000 blocks, each as signed and as unsigned; the C++ built by gcc and clang |
| `miniz` | 30,000 files totalling 800 megabytes, each compressed two or three ways and decompressed seven |
| `stb_image_write` | 9,000 images (32 million pixels), each as three files; the C built by gcc and clang at several optimisation levels |
| `stb_image` | 60,000 files: 46,500 decoded (105 million pixels) and 13,300 rejected, each for the same reason; the C built by gcc and clang at several optimisation levels |
| `stb_image_more` | 860,000 files: 750,000 decoded and 108,000 rejected, each for the same reason; the C built by gcc and clang at three optimisation levels; repeated with Rust's overflow checks enabled |
| `zstd_decode` | 20,000 frames (200 megabytes): 14,700 decoded and 5,300 rejected with the same message; the C built by gcc and clang at several optimisation levels |
| `zstd_compress` | 33,000 sources (3.4 gigabytes), every frame identical; the C built by gcc and clang |
| `zstd_compress_more` | 6,600 sources (1.6 gigabytes) at levels 4 to 22, every frame identical; the C built by gcc and clang; repeated with Rust's overflow checks enabled |
| `zstd_compress_long` | 4 sources (290 megabytes) at level 22 with long-distance matching, every frame identical, and different from the frames without it |
| `zstd_entropy` | 3.3 million rounds, slightly more than a fifth ending in a refusal, identical on both sides; the C built by gcc and clang |
| `tinyexr_write` | 48,000 images: 42,700 saved as byte-identical files (1.3 gigabytes) and 5,300 refused with the same message; a further 10,600 saved through the simplified interface; the C++ built by gcc and clang; repeated with Rust's overflow checks enabled. Every file is read back by the port, and losslessly stored data is recovered unchanged |
| `tinyexr_read` | 48,000 files (1.2 gigabytes), of which 38,200 had a header tinyexr accepts; of those, 30,100 images were read and 8,100 rejected, each with the same code and message; the C++ built by gcc and clang, and run under gcc's address sanitizer for half. In addition, the 379 sample, regression and fuzzing files in tinyexr's own repository are read identically |
| `tinyexr_parts` | 60,000 files of one to four parts: 39,000 saved as byte-identical files (199 megabytes) and 21,000 refused with the same message. Every file read identically; 167 readings depended on memory past the end and 2 crashed tinyexr, and the port rejected each of them; the C++ built by gcc and clang; repeated with Rust's overflow checks enabled |
| `tinyexr_deep` | 90,000 deep files (264 megabytes), read identically; 252 readings depended on memory outside the file and 754 crashed tinyexr or exhausted its memory, and the port rejected each of them; the C++ built by gcc and clang; repeated with Rust's overflow checks enabled |
| `tinyexr_zfp_write` | 160,000 images: 114,800 saved as byte-identical files (595 megabytes) and 45,200 refused with the same message, among them every block or tile not of whole four by four blocks and every setting asking for more than a gigabyte, which the port refuses itself |
| `tinyexr_zfp_read` | 160,000 files (505 megabytes), of which 107,000 had a header tinyexr accepts; 78,600 images read and 28,300 rejected, each with the same code and message |
| `libdeflate` | 40,000 sources (1.2 gigabytes), every stream identical, and every decompression of it, whole and damaged, with the same bytes or the same refusal |
| `dwa_write` | 400,000 blocks (7.7 gigabytes of lines) compressed to identical blocks (4.2 gigabytes): 262,000 by DWAA or DWAB and 138,000 stored as they are, not coming out smaller; every compressed block decompressed to the same lines |
| `dwa_read` | 400,000 damaged blocks (4.3 gigabytes): 69,000 decompressed to the same lines and 331,000 refused with the same code; OpenEXRCore built by gcc at `-O3`, as its CMake build has it. In addition, files written by the port are read by OpenEXRCore, and files written by OpenEXRCore by the port, to the same values |

## The ISPC originals

The two ISPC originals encode several blocks at once in the lanes of a vector register; the
ports encode them sequentially with the same arithmetic in the same order. Built for SSE2
without fast-math, each original yields the same bytes on every instruction set, at every
vector width and optimisation level, and regardless of which blocks are encoded alongside a
given block. This was established before each port began, so that there is a single
reference result to match.

## The JPEG decoder

The JPEG decoder is the one port with a choice of reference. stb_image has two
implementations of its inverse transform: plain C, and SSE2 or NEON where available. For a
file produced by an encoder they yield the same pixels (the two builds were compared on
20,000 such files). For a damaged file, whose values may be ones no encoder emits, they can
differ, because the SIMD version retains fewer intermediate bits. The port follows the plain
C implementation, which gives one result on every processor.

## Golden tests

`cargo test` requires neither a C++ compiler nor ISPC. Eighteen of its tests run synthetic
blocks, images and files through the ports and compare the resulting hashes with values
recorded from the originals: seventeen in `tests/*_golden.rs` and one in `src/bc7enc.rs`.

## Container and file interoperability

Files are checked separately by `compare/files.sh`:

```text
KTX=<checkout of KTX-Software> KTX_BUILD=<its build folder> TINYDDS=<checkout of tinyddsloader> \
    compare/files.sh
```

- **Texture containers.** It writes textures of every format and layout (plain, texture
  array, cube map, cube-map array, volume texture) as `.dds`, `.ktx` and `.ktx2`, the last
  also with ZLIB and Zstandard supercompression, and has independent readers consume them.
  The Khronos validator must pass every `.ktx2` file, and the Khronos library
  ([KTX-Software](https://github.com/KhronosGroup/KTX-Software)) and
  [tinyddsloader](https://github.com/benikabocha/tinyddsloader) must report the same
  dimensions, layers, faces and levels for each file, and the same bytes in every image.

- **Round trip.** The Khronos tool recompresses each `.ktx2` file with Zstandard, and the
  crate must read back what it originally wrote.

- **OpenEXR.** It writes `.exr` files, as scanlines and tiles, halves and floats, with each
  of the eight compression methods, and has [ffmpeg](https://ffmpeg.org), whose `.exr`
  reader is an independent implementation, read them; it must recover the same floats as the
  crate, bit for bit. Conversely, ffmpeg writes `.exr` files in every variant it supports,
  and the crate must read them identically.
