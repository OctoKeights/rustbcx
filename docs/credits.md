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

## Licences

rustbcx is distributed under the Apache License 2.0, the licence of `bc7e.ispc`: see
[`LICENSE`](../LICENSE) and [`NOTICE`](../NOTICE).

The remaining originals retain their own licences, and their notices are preserved in this
repository, under MIT, or MIT or public domain at your choice:
[`LICENSE-bc7enc_rdo.txt`](../LICENSE-bc7enc_rdo.txt),
[`LICENSE-bcdec.txt`](../LICENSE-bcdec.txt),
[`LICENSE-ispc-texcomp.txt`](../LICENSE-ispc-texcomp.txt) and
[`LICENSE-bc6h_enc.txt`](../LICENSE-bc6h_enc.txt).

## The ports in crates of their own

The image files and the supercompression of `.ktx2` are ports in crates of their own, which
rustbcx depends on. Each is offered under its original's licence, with the original's
notices, in its own repository:

| Crate | Original | Authors | Licence |
|---|---|---|---|
| [miniz_nrp](https://github.com/OctoKeights/miniz_nrp) | [miniz](https://github.com/richgel999/miniz) | Rich Geldreich and others | MIT |
| [zstd_nrp](https://github.com/OctoKeights/zstd_nrp) | [zstd](https://github.com/facebook/zstd) | Meta Platforms, Inc. and affiliates; originally by Yann Collet | BSD or GPL-2.0 |
| [stb_image_nrp](https://github.com/OctoKeights/stb_image_nrp), [stb_image_write_nrp](https://github.com/OctoKeights/stb_image_write_nrp) | [stb](https://github.com/nothings/stb) | Sean Barrett and others; the JPEG writer is derived from Jon Olick's `jo_jpeg.cpp` | MIT or public domain |
| [tinyexr_nrp](https://github.com/OctoKeights/tinyexr_nrp) | [tinyexr](https://github.com/syoyo/tinyexr) | Syoyo Fujita and many contributors, with code from OpenEXR by Industrial Light & Magic | BSD 3-Clause |
| [openexr_nrp](https://github.com/OctoKeights/openexr_nrp) | [OpenEXR](https://github.com/AcademySoftwareFoundation/openexr) | DreamWorks Animation and the Contributors to the OpenEXR Project | BSD 3-Clause |
| [zfp_nrp](https://github.com/OctoKeights/zfp_nrp) | [zfp](https://github.com/LLNL/zfp) | Lawrence Livermore National Security, LLC | BSD 3-Clause |
| [libdeflate_nrp](https://github.com/OctoKeights/libdeflate_nrp) | [libdeflate](https://github.com/ebiggers/libdeflate) | Eric Biggers and others | MIT |

The exact terms for each component are those in its notice file.

## Contributing

Bug reports and contributions are welcome. Any block that rustbcx encodes or decodes
differently from its original is a defect this project exists to eliminate: please open an
issue.
