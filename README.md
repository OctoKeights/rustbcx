# rustbcx

**Deterministic BC1–BC7 GPU texture compression in pure, safe Rust.**

**Overview** · [Examples](docs/examples.md) · [Guide](docs/guide.md) · [Behaviour](docs/behaviour.md) · [Verification](docs/verification.md) · [Performance](docs/performance.md) · [Credits](docs/credits.md)

---

rustbcx encodes and decodes block-compressed textures, from single blocks to complete
mipmapped textures in `.dds`, `.ktx` and `.ktx2` containers, sourced from image files of
every common kind or raw pixels.

Its codecs are faithful ports of established C++ and ISPC libraries, principally Richard
Geldreich's [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo), and reproduce their
output exactly with no external toolchain.

## Highlights

- **Bit-identical output.** Every block matches its reference encoder byte for byte.

- **Pure Rust all the way down.** No C++ build step, no ISPC compiler: its only
  dependencies are its sister ports for image files and supercompression, the `_nrp` crates
  below, each under its original's licence.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

- **Complete format coverage.** BC1 to BC5 and BC7 in both directions, plus signed and
  unsigned BC6H for HDR.

- **Complete textures.** Mipmapped 2D textures, arrays, cube maps and volumes, written as
  `.dds`, `.ktx` or `.ktx2`.

- **Integrated image I/O.** Reads `.png`, `.bmp`, `.jpg`, `.tga`, `.psd`, `.gif`, `.pgm`,
  `.ppm`, `.hdr` and `.exr`; writes `.png`, `.bmp`, `.jpg` and `.exr`. Through the ports of
  stb_image, stb_image_write and tinyexr.

- **Parallel and deterministic.** Uses every core by default; output is independent of the
  thread count.

- **Rate-distortion optimisation.** Smaller files after general-purpose compression, at a
  controllable cost in quality.

## Installation

```toml
[dependencies]
rustbcx = "0.4"
```

Build with `--release`: the encoders are many times slower without optimisation.

## Quick start

Compress an image to a mipmapped `.dds` file and read it back:

```rust
use rustbcx::image::Image;
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcParams};
use rustbcx::texture::{self, Texture};

// An image is a width, a height, and RGBA bytes in rows from the top.
let (width, height) = (64u32, 48u32);
let rgba: Vec<u8> = (0..width * height).flat_map(|i| [(i % width * 4) as u8, (i / width * 5) as u8, 128, 255]).collect();
let image = Image::from_rgba8(width, height, &rgba).unwrap();

let mut params = RdoBcParams::new();
params.format = BlockFormat::Bc7; // or Bc1, Bc3, Bc4, Bc5
params.bc7_uber_level = 3;        // 0 is fastest, 6 best

// Packed on all the machine's cores, every mip level down to one pixel.
let texture = texture::compress(&image, &params, true).unwrap();
assert_eq!(texture.levels.len(), 7);

// The file's ending says which kind to write: .dds, .ktx or .ktx2.
let path = std::env::temp_dir().join("rustbcx-example.dds");
texture.save(&path).unwrap();
assert_eq!(Texture::load(&path).unwrap(), texture);
```

See [Examples](docs/examples.md) for image files, HDR, cube maps and block-level access.

## Modules

| Area | Modules | Origin |
|---|---|---|
| BC1, BC3, BC4, BC5 | `rgbcx` | bc7enc_rdo |
| BC2 | `bc2` | rustbcx |
| BC7 | `bc7enc`, `bc7e`, `bc7decomp` | bc7enc_rdo, Binomial's `bc7e.ispc` |
| BC6H | `bc6h`, `bc6h_enc` | bcdec, Intel's ISPC Texture Compressor, bc6h_enc (DirectXTex) |
| Rate-distortion optimisation | `ert`, `rdo_bc_encoder` | bc7enc_rdo |
| Images, textures and containers | `image`, `texture`, `dds`, `ktx` | rustbcx |
| Image codecs | `stb_image`, `stb_image_write`, `tinyexr` | [stb_image_nrp](https://github.com/OctoKeights/stb_image_nrp), [stb_image_write_nrp](https://github.com/OctoKeights/stb_image_write_nrp), [tinyexr_nrp](https://github.com/OctoKeights/tinyexr_nrp) (with [openexr_nrp](https://github.com/OctoKeights/openexr_nrp) and [zfp_nrp](https://github.com/OctoKeights/zfp_nrp)): ports of stb, tinyexr, OpenEXR and zfp |
| Supercompression | `miniz`, `zstd` | [miniz_nrp](https://github.com/OctoKeights/miniz_nrp), [zstd_nrp](https://github.com/OctoKeights/zstd_nrp): ports of miniz and zstd |

The image codecs and supercompression are ports in crates of their own, each verified
against its original in its own repository; rustbcx gives them again under these names. Full
details are in the [module reference](docs/guide.md#module-reference).

## Licence

Apache License 2.0; see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). The ported libraries
retain their own licences, listed in [Credits](docs/credits.md).

Bug reports and contributions are welcome, above all any block that rustbcx encodes or
decodes differently from its original.
