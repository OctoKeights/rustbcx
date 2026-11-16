# Credits

[Overview](../README.md) · [Examples](examples.md) · [Guide](guide.md) · [Behaviour](behaviour.md) · [Verification](verification.md) · [Performance](performance.md) · **Credits**

---

## Attribution

All of the compression logic is the work of others; rustbcx is a translation of it into
Rust.

| Component | Authors | Source |
|---|---|---|
| `rgbcx`, `bc7enc`, `bc7decomp`, `ert`, `rdo_bc_encoder` | Richard Geldreich, Jr. | [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo) |
| `bc7e` | Binomial LLC, Copyright (C) 2018-2020 | bc7enc_rdo; first released as [bc7e](https://github.com/BinomialLLC/bc7e) |
| BC6H decoding | Sergii Kudlai | [bcdec](https://github.com/iOrange/bcdec) |
| BC6H encoding (`bc6h`) | Intel Corporation, Copyright 2017 | [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor) |
| BC6H encoding (`bc6h_enc`) | Microsoft (DirectXTex); consolidated into a single file by Alexander Samusev | [bc6h_enc](https://github.com/0xc0de/bc6h_enc) |
| zlib (`miniz`) | Rich Geldreich and others | [miniz](https://github.com/richgel999/miniz) |
| Zstandard (`zstd`) | Meta Platforms, Inc. and affiliates; originally by Yann Collet | [zstd](https://github.com/facebook/zstd) |
| `.png`, `.bmp` and `.jpg` codecs | Sean Barrett and others; the JPEG writer is derived from Jon Olick's `jo_jpeg.cpp` | [stb](https://github.com/nothings/stb) |
| `.exr` codec | Syoyo Fujita and many contributors, with code from OpenEXR by Industrial Light & Magic | [tinyexr](https://github.com/syoyo/tinyexr) |

## Licences

rustbcx is distributed under the Apache License 2.0, the licence of `bc7e.ispc`: see
[`LICENSE`](../LICENSE) and [`NOTICE`](../NOTICE).

The remaining originals retain their own licences, and their notices are preserved in this
repository:

- **MIT, or MIT or public domain at your choice:**
  [`LICENSE-bc7enc_rdo.txt`](../LICENSE-bc7enc_rdo.txt),
  [`LICENSE-bcdec.txt`](../LICENSE-bcdec.txt),
  [`LICENSE-ispc-texcomp.txt`](../LICENSE-ispc-texcomp.txt),
  [`LICENSE-bc6h_enc.txt`](../LICENSE-bc6h_enc.txt),
  [`LICENSE-miniz.txt`](../LICENSE-miniz.txt) and
  [`LICENSE-stb.txt`](../LICENSE-stb.txt).

- **BSD:** zstd, in [`LICENSE-zstd.txt`](../LICENSE-zstd.txt); and tinyexr with the OpenEXR
  code it contains, in [`LICENSE-tinyexr.txt`](../LICENSE-tinyexr.txt).

The exact terms for each component are those in its notice file.

## Contributing

Bug reports and contributions are welcome. Any block that rustbcx encodes or decodes
differently from its original is a defect this project exists to eliminate: please open an
issue.
