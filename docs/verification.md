# Verification

[Overview](../README.md) · [Examples](examples.md) · [Guide](guide.md) · [Behaviour](behaviour.md) · **Verification** · [Performance](performance.md) · [Credits](credits.md)

---

- [Differential testing against the originals](#differential-testing-against-the-originals)
- [Coverage](#coverage)
- [Results](#results)
- [The ISPC originals](#the-ispc-originals)
- [Golden tests](#golden-tests)
- [Container and file interoperability](#container-and-file-interoperability)

## Differential testing against the originals

The repository includes a harness that compares each port with its original directly.
`compare/compare.sh` builds the originals, runs synthetic blocks, images and files through
both implementations, and confirms that the results are identical byte for byte:

```text
ISPC=<the ispc compiler> BCDEC=<checkout of bcdec> ISPCTC=<checkout of ISPCTextureCompressor> \
    BC6HENC=<checkout of bc6h_enc> \
    compare/compare.sh <checkout of bc7enc_rdo> [blocks] [seeds] [dumps]
```

It requires a C++ compiler, the [ISPC compiler](https://github.com/ispc/ispc/releases) for
the two ISPC originals, and the checkouts named above. Anything it cannot build is reported
as not compared, and the run fails.

The image files and the supercompression of `.ktx2` are ports in crates of their own, each
with its own comparison with its original in its `compare/compare.sh`, its own golden tests,
and its own record of results in its README:
[miniz_nrp](https://github.com/OctoKeights/miniz_nrp),
[zstd_nrp](https://github.com/OctoKeights/zstd_nrp),
[stb_image_nrp](https://github.com/OctoKeights/stb_image_nrp),
[stb_image_write_nrp](https://github.com/OctoKeights/stb_image_write_nrp),
[tinyexr_nrp](https://github.com/OctoKeights/tinyexr_nrp),
[openexr_nrp](https://github.com/OctoKeights/openexr_nrp),
[zfp_nrp](https://github.com/OctoKeights/zfp_nrp) and
[libdeflate_nrp](https://github.com/OctoKeights/libdeflate_nrp).

## Coverage

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

## Results

The comparisons were run on Linux (x86-64), with the C++ built by gcc and the ISPC by ISPC
1.32.0. All results were identical.

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

## The ISPC originals

The two ISPC originals encode several blocks at once in the lanes of a vector register; the
ports encode them sequentially with the same arithmetic in the same order. Built for SSE2
without fast-math, each original yields the same bytes on every instruction set, at every
vector width and optimisation level, and regardless of which blocks are encoded alongside a
given block. This was established before each port began, so that there is a single
reference result to match.

## Golden tests

`cargo test` requires neither a C++ compiler nor ISPC. Seven of its tests run synthetic
blocks, images and files through the ports and compare the resulting hashes with values
recorded from the originals: six in `tests/*_golden.rs` and one in `src/bc7enc.rs`.

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
