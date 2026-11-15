# rustbcx

Texture block compression in pure Rust, BC1 to BC5 and BC7: a port of Richard Geldreich's
[bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo) and of the encoder that comes with it,
Binomial's `bc7e.ispc`.

The goal of this crate is simple: pack and unpack GPU texture blocks exactly the way those
libraries do, in safe Rust, with nothing else to install.

- **Bit-identical output.** The port follows the originals function by function and keeps
  their floating-point operations in the same order, so the blocks you get are the same bytes
  the originals produce.
- **No dependencies.** The crate uses only the Rust standard library. There is no C++ to
  build and no ISPC compiler to install.
- **No `unsafe` code.** The crate is built with `#![forbid(unsafe_code)]`.
- **Every format both ways.** BC1, BC3, BC4, BC5 and BC7, from pixels to blocks and back.
- **Two BC7 encoders.** A small, quick one, and a thorough one that uses all eight of BC7's
  modes.

## Contents

- [Getting started](#getting-started)
- [What is in it](#what-is-in-it)
- [Examples](#examples)
- [Which BC7 encoder](#which-bc7-encoder)
- [Is this the right crate for you?](#is-this-the-right-crate-for-you)
- [Things to know](#things-to-know)
- [How the port is verified](#how-the-port-is-verified)
- [Speed](#speed)
- [Limitations](#limitations)
- [Credits and licence](#credits-and-licence)

## Getting started

Put it in your `Cargo.toml`, straight from this repository:

```toml
[dependencies]
rustbcx = { git = "https://github.com/OctoKeights/rustbcx" }
```

A block is sixteen RGBA pixels, left to right and top to bottom, each pixel a `[u8; 4]`. Split
your image into 4x4 blocks, pack each one, and write the packed blocks out in the same order:
that is the texture data a `.dds` or `.ktx` file holds.

## What is in it

| Module | Port of | What it does |
|---|---|---|
| `rgbcx` | `rgbcx.cpp` | BC1, BC3, BC4 and BC5: pixels to blocks and back |
| `bc7decomp` | `bc7decomp.cpp` | BC7 blocks of every mode to pixels |
| `bc7enc` | `bc7enc.cpp` | Pixels to BC7, the small and quick encoder: modes 1 and 6 for opaque blocks, modes 5, 6 and 7 for blocks with alpha |
| `bc7e` | `bc7e.ispc` | Pixels to BC7, the thorough encoder: all eight modes, seven profiles from fastest to best |

## Examples

BC7 with the thorough encoder:

```rust
use rustbcx::bc7e::{Bc7e, CompressBlockParams};

// Makes the encoder's tables: once, then share it.
let encoder = Bc7e::new();

// One of the seven profiles, for error measured as the eye sees it (true) or in plain RGB.
let params = CompressBlockParams::slow(true);

let pixels = [[200u8, 120, 40, 255]; 16];
let mut block = [0u8; 16];
encoder.compress_block(&mut block, &pixels, &params);

// Or many at once.
let mut blocks = vec![[0u8; 16]; 4];
encoder.compress_blocks(&mut blocks, &[pixels; 4], &params);
```

BC7 with the quick encoder, and back to pixels:

```rust
use rustbcx::bc7decomp;
use rustbcx::bc7enc::{Bc7Enc, CompressBlockParams};

let encoder = Bc7Enc::new();
let params = CompressBlockParams::new();

let pixels = [[200u8, 120, 40, 255]; 16];
let mut block = [0u8; 16];
encoder.compress_block(&mut block, &pixels, &params);

let mut unpacked = [[0u8; 4]; 16];
assert!(bc7decomp::unpack_bc7(&block, &mut unpacked));
```

BC1 (BC3, BC4 and BC5 are alike):

```rust
use rustbcx::rgbcx::{self, Bc1ApproxMode, Rgbcx};

let encoder = Rgbcx::new(Bc1ApproxMode::Ideal);

let pixels = [[200u8, 120, 40, 255]; 16];
let mut block = [0u8; 8];
// Level 0 is fastest and 18 best; no three-colour blocks, no transparent texels for black.
encoder.encode_bc1(10, &mut block, &pixels, false, false, None);

let mut unpacked = [[0u8; 4]; 16];
rgbcx::unpack_bc1(&block, &mut unpacked, true, Bc1ApproxMode::Ideal);
```

`Bc7e`, `Bc7Enc` and `Rgbcx` hold only tables, so one of each can be shared between threads;
split the blocks between them.

## Which BC7 encoder

- **`bc7e`** when quality matters. It can use every mode, so it does better on blocks the two
  or three modes of `bc7enc` suit poorly. Its profiles are the original's, fastest first:
  `ultrafast`, `veryfast`, `fast`, `basic`, `slow`, `veryslow`, `slowest`.
  `CompressBlockParams::new` gives the settings they all start from, and every field can be set
  by hand: which modes may be used, how many partitions are estimated and how many tried in
  full, the weights of the four channels, the uber level, the refinement passes.
- **`bc7enc`** when the blocks have to match what bc7enc_rdo's C++ gives, or for a small
  encoder with a few settings.

The two give different blocks for the same pixels: each matches its own original.

## Is this the right crate for you?

Rust already has ways to pack these formats, among them bindings to the C++ and ISPC encoders
themselves (for example [rgbcx-sys](https://crates.io/crates/rgbcx-sys) and
[intel_tex_2](https://crates.io/crates/intel_tex_2)). If you are happy to build C++ or ISPC
code as part of your project and want the most speed, one of those may suit you better: the
ISPC encoders pack several blocks at once with SIMD, and this crate packs one at a time.

rustbcx is aimed at a narrower need:

- You are moving a C++ project that uses bc7enc_rdo to Rust, and its textures must not change.
- You want the same blocks on every machine and from every build, which the usual fast-math
  build of `bc7e.ispc` does not give.
- You would like a small, self-contained encoder and decoder with no dependencies at all.

The crate was written for Noolith, a project whose C++ code packed its textures with
bc7enc_rdo and whose Rust rewrite needed to produce exactly the same blocks.

## Things to know

- **With `bc7e`, an opaque block can unpack with alpha 254.** Mode 6 has alpha, and for a block
  without transparency the encoder does not count alpha when it chooses the endpoints' last
  bits. Some or all pixels of such a block then unpack as 254, not 255. The original does the
  same. A shader that ignores alpha for opaque textures never sees it; to rule it out, switch
  mode 6 off for opaque blocks (`opaque_settings.use_mode[6] = false`).
- **`bc7e` matches the strict build of the original, not the usual one.** `bc7e.ispc` is
  normally compiled with `--opt=fast-math` for several instruction sets, which lets the compiler
  reorder the arithmetic. That build packs about one block in eleven differently from the strict
  one, in the last bits of a choice and not in kind. This crate gives the strict build's blocks.
- **Oddities of the originals are kept**, since changing them would change the blocks. In
  `bc7e`, mode 7's table of best endpoints for a single colour carries one p-bit pair's find
  over to the next, and its single-colour error is worked out from the six-bit values without
  widening them.
- **With `bc7e`, solid blocks are exact.** A block of one colour is packed in mode 5 and unpacks
  to exactly that colour, alpha included.
- **`bc7enc` panics when its mode mask leaves a block no mode**, where the C++ asserts.
- **The originals' globals are values here.** The tables the C++ and the ISPC fill at start-up
  are held by `Rgbcx::new`, `Bc7Enc::new` and `Bc7e::new`, so there is no init function to
  remember to call.

## How the port is verified

The repository includes a tool that checks the port against the originals directly.
`compare/compare.sh` builds them from a checkout of bc7enc_rdo, runs made-up blocks through
them and through this crate, and confirms that the results match byte for byte:

```text
compare/compare.sh <checkout of bc7enc_rdo> [blocks] [seeds] [dumps]
```

**Against the C++ library** (`bc7decomp.cpp`, `bc7enc.cpp`, `rgbcx.cpp`, built without
contracting a multiply and an add into one instruction, which Rust never does):

- `bc7decomp`: BC7 blocks of every mode, and of none, unpacked.
- `unpack`: BC1, BC3, BC4 and BC5 blocks unpacked every way, and solid BC1 blocks made.
- `bc1`, `bc345`: made-up pixels packed at every level and with every flag, for each way a GPU
  interpolates, and unpacked again.
- `bc7enc`: the same pixels packed as BC7 with every setting varied, and unpacked again.
- `bc7block`: BC7 blocks of every mode written from made-up endpoints and selectors.

**Against `bc7e.ispc`**, which needs the [ISPC compiler](https://github.com/ispc/ispc/releases)
(`ispc` on the path, or named in `$ISPC`). The original packs several blocks at once in the
lanes of a vector register; the port packs them one after another with the same arithmetic in
the same order. Built for SSE2 without fast-math, the original gives the same bytes on every
instruction set, at every vector width and optimisation level, and whatever blocks are packed
beside a block (tested before the port was begun), so there is one answer to match.

- `bc7e_presets`: the same blocks with each of the seven profiles, for both error measures.
- `bc7e_settings`: the blocks in groups of 32, each group with settings made up from the seed
  (modes switched on and off, partition counts, weights, masks, uber levels, refinement passes).

The made-up blocks are noise, ramps, greys, single colours, dark blocks and blocks of a few
colours, with and without alpha.

### Results so far

These comparisons were run on Linux (x86-64).

| Compared | Result |
|---|---|
| Every dump against the C++ library built with gcc | Identical |
| `bc7enc`: 3 million blocks with every setting varied, and 3 million written in all eight modes; the C++ built by gcc and by clang at several optimisation levels; again with Rust's overflow checks on | Identical |
| `bc7e` against the strict build made by ISPC 1.32.0: 1.68 million blocks through the seven profiles, 780,000 through about 24,000 made-up settings; a part of it again with Rust's overflow checks on | Identical, with every mode used |

`cargo test` needs neither a C++ compiler nor ISPC. Among its tests, two pack made-up blocks
and compare their hashes with ones recorded from the originals: `tests/bc7e_golden.rs` for
`bc7e` (7,200 blocks) and one in `src/bc7enc.rs` for `bc7enc`.

## Speed

For `bc7e`, on one thread of the machine it was written on, 70,000 blocks (5,000 through each
profile and error measure) took about 4.2 seconds with this crate, 4.4 seconds in the strict
SSE2 build of the original, and 3.0 seconds in its AVX2 fast-math build. Take that as a rough
guide only: it is one machine and made-up blocks.

The crate does no threading of its own. Blocks are independent, so the simple way to use more
cores is to give each thread its own run of blocks.

## Limitations

- **No rate-distortion pass.** bc7enc_rdo can also trade a little quality for blocks that
  compress better afterwards (the "rdo" of its name). That part is not ported.
- **Blocks only.** The crate packs and unpacks 4x4 blocks. Reading and writing `.dds` or `.ktx`
  files, making mipmaps and splitting an image into blocks are left to you.
- **One block at a time.** There is no SIMD across blocks as in the ISPC original.
- **BC6H is not here**, as it is not in bc7enc_rdo.

## Credits and licence

All of the compression logic is the work of others. `rgbcx`, `bc7enc` and `bc7decomp` are by
Richard Geldreich, Jr., from [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo). `bc7e` is
by Binomial LLC, Copyright (C) 2018-2020, from the same repository (first released as
[bc7e](https://github.com/BinomialLLC/bc7e)). This crate is a translation of that work into
Rust.

rustbcx is available under the Apache License 2.0, the licence of `bc7e.ispc`: see
[`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). bc7enc_rdo's own files are offered by their author
under the MIT licence or as public domain; that notice is kept in
[`LICENSE-bc7enc_rdo.txt`](LICENSE-bc7enc_rdo.txt).

Bug reports and contributions are welcome. If you find a block that this crate packs or
unpacks differently from the originals, please open an issue: that is exactly the kind of bug
this project exists to rule out.
