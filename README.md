# rustbcx

**Deterministic BC1–BC7 GPU texture compression in pure, safe Rust.**

**Overview** · [Examples](docs/examples.md) · [Guide](docs/guide.md) · [Behaviour](docs/behaviour.md) · [Verification](docs/verification.md) · [Performance](docs/performance.md) · [Credits](docs/credits.md)

---

rustbcx encodes and decodes block-compressed textures, from individual blocks to complete
mipmapped textures in `.dds`, `.ktx` and `.ktx2` containers. Source images may be `.png`,
`.bmp`, `.jpg` or `.exr` files, or raw pixel data.

Its codecs are faithful ports of established C++ and ISPC libraries, principally Richard
Geldreich's [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo). The crate has a single
objective: to reproduce the output of those libraries exactly, in safe Rust, with no external
toolchain.

## Highlights

- **Bit-identical output.** Each port mirrors its original function by function and preserves
  the order of floating-point operations, so every block matches the reference byte for byte.
- **Zero dependencies.** Only the Rust standard library is required: no C++ build step and no
  ISPC compiler.
- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.
- **Complete format coverage.** BC1, BC2, BC3, BC4, BC5 and BC7 in both directions, and BC6H
  for HDR colour, signed and unsigned.
- **Complete textures.** Mipmapped 2D textures, texture arrays, cube maps and volume textures,
  written as `.dds`, `.ktx` or `.ktx2`.
- **Integrated image I/O.** `.png`, `.bmp` and `.jpg` through ports of stb_image and
  stb_image_write; `.exr` through a port of tinyexr.
- **Parallel and deterministic.** Encoding uses every available core by default, and the
  output is independent of the thread count.
- **Rate-distortion optimisation.** The bc7enc_rdo pass that reduces a texture's size under
  general-purpose compression, at a small and controllable cost in quality.

## Installation

```toml
[dependencies]
rustbcx = "0.3"
```

Compile with optimisation (`cargo build --release`). The encoders are arithmetic-intensive
and are many times slower in debug builds.

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

Further examples, covering image files, HDR, cube maps and block-level access, are in
[Examples](docs/examples.md).

## Modules

| Area | Modules | Origin |
|---|---|---|
| BC1, BC3, BC4, BC5 | `rgbcx` | bc7enc_rdo |
| BC2 | `bc2` | rustbcx |
| BC7 | `bc7enc`, `bc7e`, `bc7decomp` | bc7enc_rdo, Binomial's `bc7e.ispc` |
| BC6H | `bc6h`, `bc6h_enc` | bcdec, Intel's ISPC Texture Compressor, bc6h_enc (DirectXTex) |
| Rate-distortion optimisation | `ert`, `rdo_bc_encoder` | bc7enc_rdo |
| Images, textures and containers | `image`, `texture`, `dds`, `ktx` | rustbcx |
| Image codecs | `stb_image`, `stb_image_write`, `tinyexr` | stb, tinyexr |
| Supercompression | `miniz`, `zstd` | miniz, zstd |

The full module reference is in the [Guide](docs/guide.md#module-reference).

## Documentation

| Page | Contents |
|---|---|
| [Examples](docs/examples.md) | Worked examples for each principal workflow |
| [Guide](docs/guide.md) | Module reference, encoder selection, rate-distortion optimisation, threading, suitability |
| [Behaviour](docs/behaviour.md) | Behavioural notes and known limitations |
| [Verification](docs/verification.md) | How each port is validated against its original, with results |
| [Performance](docs/performance.md) | Benchmarks and the optional `nightly-simd` feature |
| [Credits](docs/credits.md) | Attribution and third-party licences |

## Licence

rustbcx is distributed under the Apache License 2.0; see [`LICENSE`](LICENSE) and
[`NOTICE`](NOTICE). The ported libraries retain their own licences, detailed in
[Credits](docs/credits.md).

Bug reports and contributions are welcome. Any block that rustbcx encodes or decodes
differently from its original is a defect this project exists to eliminate: please open an
issue.
