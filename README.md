# rustbcx

GPU texture compression in pure Rust: BC1 to BC7, from single blocks up to whole textures with
mip levels in `.dds`, `.ktx` and `.ktx2` files, made from `.png`, `.bmp` and `.jpg` pictures or
from pixels of your own.

The encoders and decoders are ports of well-known C++ and ISPC libraries, chiefly Richard
Geldreich's [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo), and the goal of this crate
is simple: pack and unpack texture blocks exactly the way those libraries do, in safe Rust,
with nothing else to install.

- **Bit-identical output.** Each port follows its original function by function and keeps its
  floating-point operations in the same order, so the blocks you get are the same bytes the
  original produces.
- **No dependencies.** The crate uses only the Rust standard library. There is no C++ to
  build and no ISPC compiler to install.
- **No `unsafe` code.** The crate is built with `#![forbid(unsafe_code)]`.
- **Every format.** BC1, BC2, BC3, BC4, BC5 and BC7 both ways, and BC6H for HDR colour, signed
  and unsigned.
- **Whole textures.** Hand it an image and get back a texture with its mip levels, ready to
  write as a `.dds`, `.ktx` or `.ktx2` file. Hand it several and get a texture array, a cube
  map or a volume texture.
- **Pictures in and out.** It reads `.png`, `.bmp` and `.jpg` files into images and writes
  images back out as them, with ports of stb_image and stb_image_write.
- **All your cores.** Images are packed on every core the machine has unless you say
  otherwise, and the result does not depend on how many that is.
- **Rate-distortion optimisation.** bc7enc_rdo's pass that makes a texture smaller on disk
  once it is zipped, at a small and controllable cost in quality.

## Contents

- [Getting started](#getting-started)
- [What is in it](#what-is-in-it)
- [Examples](#examples)
- [Which BC7 encoder](#which-bc7-encoder)
- [The rate-distortion pass](#the-rate-distortion-pass)
- [Threads](#threads)
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

Build your own project with optimisation (`cargo build --release`): the encoders do a great
deal of arithmetic and are many times slower without it.

## What is in it

The ports, each checked byte for byte against its original:

| Module | Port of | What it does |
|---|---|---|
| `rgbcx` | bc7enc_rdo's `rgbcx.cpp` | BC1, BC3, BC4 and BC5: pixels to blocks and back |
| `bc7decomp` | bc7enc_rdo's `bc7decomp.cpp` | BC7 blocks of every mode to pixels |
| `bc7enc` | bc7enc_rdo's `bc7enc.cpp` | Pixels to BC7, the small and quick encoder: modes 1 and 6 for opaque blocks, modes 5, 6 and 7 for blocks with alpha |
| `bc7e` | Binomial's `bc7e.ispc` | Pixels to BC7, the thorough encoder: all eight modes, seven profiles from fastest to best |
| `ert` | bc7enc_rdo's `ert.cpp` | The rate-distortion pass over packed blocks of any of these formats |
| `rdo_bc_encoder` | bc7enc_rdo's `rdo_bc_encoder.cpp` | A whole image to blocks of BC1, BC3, BC4, BC5 or BC7, with the rate-distortion pass (and of BC2, which is this crate's addition) |
| `bc6h` (unpacking) | [bcdec](https://github.com/iOrange/bcdec)'s `bcdec.h` | BC6H blocks, signed and unsigned, to half floats or floats |
| `bc6h` (packing) | Intel's [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor), `kernel.ispc` | HDR pixels to unsigned BC6H, five profiles |
| `bc6h_enc` | [bc6h_enc](https://github.com/0xc0de/bc6h_enc), which is DirectXTex's BC6H encoder as one file | HDR pixels to signed or unsigned BC6H, the thorough and slow way |
| `miniz` | Rich Geldreich's [miniz](https://github.com/richgel999/miniz): its compressor, its decompressor and its checksums | zlib and deflate, both ways, for `.ktx2` files with ZLIB supercompression |
| `stb_image` | Sean Barrett's [stb](https://github.com/nothings/stb), `stb_image.h`: its PNG, BMP and JPEG readers and its zlib decoder | `.png`, `.bmp` and `.jpg` files to pixels, at 8 or 16 bits a channel |
| `stb_image_write` | stb's `stb_image_write.h`: its PNG, BMP and JPEG writers | Pixels of 8 bits a channel to `.png`, `.bmp` and `.jpg` files |

And around them, the crate's own code, which has no original to match:

| Module | What it does |
|---|---|
| `bc2` | BC2 (DXT3), which none of the originals has: the colours by `rgbcx` as BC3's are, the alphas at four bits a pixel. Its unpacker is checked against bcdec's |
| `image` | `Image` (8-bit RGBA) and `HdrImage` (half-float RGB), and the next mip level of each. `Image::load` and `save` read and write `.png`, `.bmp` and `.jpg` files |
| `texture` | `Texture`: a format, a size and the blocks of every mip level, for one image or for a texture array, a cube map or a volume texture. `compress` and `compress_hdr` make one from an image, `compress_many` and `compress_hdr_many` from several; `load` and `save` read and write files |
| `dds` | `.dds` files, both ways, with either header. For one level its writer gives the bytes bc7enc_rdo's `save_dds` writes |
| `ktx` | `.ktx` (KTX 1) and `.ktx2` (KTX 2) files, both ways; `.ktx2` with its levels as they are or supercompressed with zlib |

## Examples

An image to a `.dds` file with mip levels, and back:

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

A picture file in, and one out:

```rust
use rustbcx::image::Image;
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcParams};
use rustbcx::rgbcx::Bc1ApproxMode;
use rustbcx::texture;

// (A picture to read: here one is made and saved first.)
let folder = std::env::temp_dir();
let mut picture = Image::new(32, 32);
for (i, pixel) in picture.pixels.iter_mut().enumerate() {
    *pixel = [(i % 32 * 8) as u8, (i / 32 * 8) as u8, 60, 255];
}
picture.save(folder.join("rustbcx-example.png")).unwrap();

// A .png, .bmp or .jpg file comes in as an image...
let image = Image::load(folder.join("rustbcx-example.png")).unwrap();
assert_eq!(image, picture);

let mut params = RdoBcParams::new();
params.format = BlockFormat::Bc7;
let texture = texture::compress(&image, &params, false).unwrap();

// ...and what a texture unpacks to can go out as one, to see what the packing did.
let unpacked = texture.unpack_level(0, Bc1ApproxMode::Ideal).unwrap();
unpacked.save(folder.join("rustbcx-example-unpacked.png")).unwrap();
```

The same with the rate-distortion pass, for a file that zips smaller:

```rust
use rustbcx::image::Image;
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcParams};
use rustbcx::texture;

let image = Image::new(64, 64); // (yours, not a black one)

let mut params = RdoBcParams::new();
params.format = BlockFormat::Bc1;
params.rdo_lambda = 1.0; // 0 is off; higher gives smaller files and more error

let texture = texture::compress(&image, &params, false).unwrap();
let dds_file: Vec<u8> = rustbcx::dds::write(&texture, false).unwrap();
assert_eq!(dds_file.len(), 128 + 16 * 16 * 8);
```

An HDR image to BC6H:

```rust
use rustbcx::bc6h::Bc6hEncSettings;
use rustbcx::image::HdrImage;
use rustbcx::texture::{self, TextureFormat};

// Floats, three to a pixel: red, green, blue.
let (width, height) = (32u32, 32u32);
let rgb: Vec<f32> = (0..width * height).flat_map(|i| [1.0 + (i % width) as f32 * 0.05, 2.0, 0.5]).collect();
let image = HdrImage::from_rgb_f32(width, height, &rgb).unwrap();

// One of five profiles, with mip levels, on all cores (the last argument: 0).
let texture = texture::compress_hdr(&image, &Bc6hEncSettings::basic(), true, 0).unwrap();
assert_eq!(texture.format, TextureFormat::Bc6hUnsigned);

// Back to an image of floats.
let unpacked = texture.unpack_level_hdr(0).unwrap().to_rgb_f32();
assert_eq!(unpacked.len(), rgb.len());
```

For values below zero there is signed BC6H: `HdrImage::from_rgb_f32_signed` keeps them, and
`texture::compress_hdr_signed` packs them, with the thorough encoder of `bc6h_enc`.

Six images to a cube map (a texture array and a volume texture go the same way):

```rust
use rustbcx::image::Image;
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcParams};
use rustbcx::texture::{self, Layout};

// The faces in the order +X, -X, +Y, -Y, +Z, -Z, each square and all one size.
let faces: Vec<Image> = (0..6u8).map(|face| Image::from_pixels(16, 16, vec![[face * 40, 90, 200, 255]; 256]).unwrap()).collect();

let mut params = RdoBcParams::new();
params.format = BlockFormat::Bc1;
let cube = texture::compress_many(&faces, Layout::Cube, &params, true).unwrap();
assert!(cube.cube);
assert_eq!((cube.levels.len(), cube.images_per_level(0)), (5, 6));

// One face of one level, back as an image.
let face = cube.unpack_image(0, 3, rustbcx::rgbcx::Bc1ApproxMode::Ideal).unwrap();
assert_eq!((face.width, face.height), (16, 16));
```

Block by block, when you want to do the rest yourself. A block is sixteen pixels, left to right
and top to bottom:

```rust
use rustbcx::bc7decomp;
use rustbcx::bc7e::{Bc7e, CompressBlockParams};
use rustbcx::rgbcx::{self, Bc1ApproxMode, Rgbcx};

let pixels = [[200u8, 120, 40, 255]; 16];

// BC7 with the thorough encoder. Making it builds its tables: once, then share it.
let bc7_encoder = Bc7e::new();
let mut bc7_block = [0u8; 16];
bc7_encoder.compress_block(&mut bc7_block, &pixels, &CompressBlockParams::slow(true));

let mut unpacked = [[0u8; 4]; 16];
assert!(bc7decomp::unpack_bc7(&bc7_block, &mut unpacked));

// BC1. Level 0 is fastest and 18 best; no three-colour blocks, no transparent texels for black.
let bc1_encoder = Rgbcx::new(Bc1ApproxMode::Ideal);
let mut bc1_block = [0u8; 8];
bc1_encoder.encode_bc1(10, &mut bc1_block, &pixels, false, false, None);
rgbcx::unpack_bc1(&bc1_block, &mut unpacked, true, Bc1ApproxMode::Ideal);
```

## Which BC7 encoder

- **`bc7e`** when quality matters. It can use every mode, so it does better on blocks the two
  or three modes of `bc7enc` suit poorly. Its profiles are the original's, fastest first:
  `ultrafast`, `veryfast`, `fast`, `basic`, `slow`, `veryslow`, `slowest`. It is what
  `RdoBcParams` uses unless `use_bc7e` is switched off, as bc7enc_rdo does when built with it.
- **`bc7enc`** when the blocks have to match what bc7enc_rdo gives without ISPC, or for a
  small encoder with a few settings.

The two give different blocks for the same pixels: each matches its own original.

## The rate-distortion pass

A packed texture is usually compressed again on disk or in a download (zip, zstd, LZMA).
bc7enc_rdo's pass goes over the packed blocks and, where it costs little, copies runs of bytes
from the blocks just before, which is what such compressors find and shrink. Set
`RdoBcParams::rdo_lambda` above 0 to turn it on: around 0.5 to 2 is a mild trade, and higher
values give smaller files and more visible error. The pass looks after smooth regions, which
show changes most, by itself.

One setting changes the blocks and is easy to miss: an image of 4,096 blocks or more is put
through the pass in `rdo_max_threads` strips, each on its own, since a strip cannot copy from
the one before. It is 128 unless you change it, on every machine, so the same image gives the
same texture everywhere. (bc7enc_rdo's own command line sets it from the machine's core count
instead.)

## Threads

Packing an image (`texture::compress`, `texture::compress_hdr`, `RdoBcEncoder`) and packing
many blocks at once (`Bc7e::compress_blocks`, `bc6h::compress_blocks_bc6h`) use every core the
machine has. To hold them to fewer, set `RdoBcParams::threads`, pass a thread count to
`compress_hdr`, or call the `_on` forms of the many-blocks functions; 1 keeps the work on the
calling thread. The blocks are the same whatever the number.

The threads are the standard library's and live only for the call.

## Is this the right crate for you?

Rust already has ways to pack these formats, among them bindings to the C++ and ISPC encoders
themselves (for example [rgbcx-sys](https://crates.io/crates/rgbcx-sys) and
[intel_tex_2](https://crates.io/crates/intel_tex_2)). If you are happy to build C++ or ISPC
code as part of your project and want the most speed from each core, one of those may suit you
better: the ISPC encoders pack several blocks at once with SIMD instructions, and this crate
packs one block at a time on each core.

rustbcx is aimed at a narrower need:

- You are moving a C++ project that uses bc7enc_rdo to Rust, and its textures must not change.
- You want the same blocks on every machine and from every build, which the usual fast-math
  builds of the ISPC encoders do not give.
- You would like a self-contained texture compressor with no dependencies at all, and no
  `unsafe`.

The crate was written for Noolith, a project whose C++ code packed its textures with
bc7enc_rdo and whose Rust rewrite needed to produce exactly the same blocks.

## Things to know

- **With `bc7e`, an opaque block can unpack with alpha 254.** Mode 6 has alpha, and for a block
  without transparency the encoder does not count alpha when it chooses the endpoints' last
  bits. Some or all pixels of such a block then unpack as 254, not 255. The original does the
  same. A shader that ignores alpha for opaque textures never sees it; to rule it out, switch
  mode 6 off for opaque blocks (`opaque_settings.use_mode[6] = false`).
- **The ISPC ports match the strict builds of their originals, not the usual ones.**
  `bc7e.ispc` and Intel's `kernel.ispc` are normally compiled with `--opt=fast-math`, which
  lets the compiler reorder the arithmetic. Those builds pack about one block in ten
  differently from the strict ones, in the last bits of a choice and not in kind. This crate
  gives the strict builds' blocks.
- **BC6H has two packers.** `compress_hdr` uses Intel's, which is quick and packs the unsigned
  kind only, values below zero as zero. `compress_hdr_signed` uses DirectXTex's, which packs
  the signed kind and is far slower. Both kinds unpack.
- **BC6H's packer is weak on very bright blocks.** The original keeps each pixel's squared
  error as an int, which bright values overflow, so among such blocks its choices are less
  good than its search would allow. That is kept, since the blocks depend on it.
- **Oddities of the originals are kept**, since changing them would change the blocks. In
  `bc7e`, mode 7's table of best endpoints for a single colour carries one p-bit pair's find
  over to the next. In the image encoder, the rate-distortion pass compares BC4 against the
  image's red, and BC5 against red and green, whatever channels were packed.
- **`bc7enc` panics when its mode mask leaves a block no mode**, where the C++ asserts.
- **Picture files are read as stb_image reads them.** Decoders differ a little in the pixels
  they make of a `.jpg`; these are stb_image's. A file it refuses is refused here, with its
  reason in the `LoadError`, and a damaged file it reads all the same is read here too. The
  checksums in a `.png` are not checked, as there. `Image` holds 8 bits a channel, so a 16-bit
  `.png` is brought down to 8 on the way in; `stb_image::load_16_from_memory` keeps all 16.
- **A `.jpg` is written as stb_image_write writes it**: baseline, with the colour at half size
  each way up to quality 90 and at full size above it.
- **The originals' globals are values here.** The tables the C++ and the ISPC fill at start-up
  are held by `Rgbcx::new`, `Bc7Enc::new` and `Bc7e::new`, so there is no init function to
  remember to call, and one of each can be shared between threads.
- **Mip levels are plain averages** of two by two pixels, of the values as stored (of floats,
  for HDR). If you need another filter, or averaging in linear light for sRGB images, make the
  levels yourself and pack each with `RdoBcEncoder`.

## How the port is verified

The repository includes a tool that checks the ports against the originals directly.
`compare/compare.sh` builds the originals, runs made-up blocks and images through them and
through this crate, and confirms that the results match byte for byte:

```text
ISPC=<the ispc compiler> BCDEC=<checkout of bcdec> ISPCTC=<checkout of ISPCTextureCompressor> \
    BC6HENC=<checkout of bc6h_enc> MINIZ=<checkout of miniz> STB=<checkout of stb> \
    compare/compare.sh <checkout of bc7enc_rdo> [blocks] [seeds] [dumps]
```

It needs a C and a C++ compiler; the [ISPC compiler](https://github.com/ispc/ispc/releases) for the two
ISPC originals; and the folders named above. What it cannot build it says it has not compared,
and fails.

| Dump | Against | What is compared |
|---|---|---|
| `bc7decomp` | bc7enc_rdo | BC7 blocks of every mode, and of none, unpacked |
| `unpack` | bc7enc_rdo | BC1, BC3, BC4 and BC5 blocks unpacked every way |
| `bc1`, `bc345` | bc7enc_rdo | Pixels packed at every level and with every flag, for each way a GPU interpolates |
| `bc7enc` | bc7enc_rdo | Pixels packed as BC7 with every setting varied |
| `bc7block` | bc7enc_rdo | BC7 blocks of every mode written from made-up endpoints and selectors |
| `ert` | bc7enc_rdo | Runs of near-copied blocks in five formats through the rate-distortion pass, its settings varied |
| `rdo` | bc7enc_rdo | Whole images of every shape through the image encoder in each format, with and without the pass, and the `.dds` file for each |
| `rdo_bc7e` | bc7enc_rdo with `bc7e.ispc` | The same, BC7 packed by bc7e for half the images |
| `bc7e_presets`, `bc7e_settings` | `bc7e.ispc`, strict build | The seven profiles with both error measures; then about 24,000 made-up settings |
| `bc2` | bc7enc_rdo's rgbcx and bcdec | BC2 blocks unpacked (the alphas against bcdec, the colours against rgbcx's BC1), and pixels packed at every level and with every flag |
| `bc6h_unpack` | bcdec | BC6H blocks of every mode, and of none, unpacked as signed and unsigned |
| `bc6h_pack` | Intel's `kernel.ispc`, strict build | HDR pixels packed with each of the five profiles |
| `bc6h_enc` | bc6h_enc, built with `BC6H_SSE_INTRINSICS` | HDR pixels of both signs packed as signed and as unsigned BC6H |
| `miniz` | miniz | Made-up files compressed at every level, with every strategy and with made-up flags, in one go and in pieces with flushes between; each stream decompressed whole, into too little room, damaged, cut short and with bytes after it |
| `stb_image_write` | stb's `stb_image_write.h` | Made-up images of one to four channels written as `.png` with made-up compression levels and filters, as `.bmp`, and as `.jpg` with made-up qualities, some upside down and some with rows further apart than they are wide; and made-up data through its zlib compressor |
| `stb_image` | stb's `stb_image.h`, built with `STBI_NO_SIMD` | Made-up files asked their size and read at 8 and 16 bits a channel, with every number of channels asked for: pictures written by stb_image_write, and files put together with every kind of header the readers take and made-up bytes for the picture (PNG of every depth and colour type, with palettes, transparent colours and interlacing; BMP of every header size and depth, with palettes and bit masks; JPEG baseline and progressive, of one to four components sampled every way, with restart markers). A third of them are damaged first, and for those the reason for refusing is compared. And made-up zlib streams, whole and damaged, through its zlib decoder |

The two ISPC originals pack several blocks at once in the lanes of a vector register; the ports
pack them one after another with the same arithmetic in the same order. Built for SSE2 without
fast-math, each original gives the same bytes on every instruction set, at every vector width
and optimisation level, and whatever blocks are packed beside a block. That was tested before
each port was begun, so there is one answer to match.

### Results so far

These comparisons were run on Linux (x86-64), the C++ built with gcc and the ISPC with ISPC
1.32.0. All identical:

| Compared | How much |
|---|---|
| `bc7enc` | 3 million blocks with every setting varied, 3 million written in all eight modes; the C++ built by gcc and by clang at several optimisation levels; again with Rust's overflow checks on |
| `bc7e` | 1.68 million blocks through the seven profiles, 780,000 through about 24,000 made-up settings |
| `ert` | 160,000 blocks in runs of five formats |
| `rdo`, `rdo_bc7e` | Over 100 seeds of images, 2,500 to 10,000 blocks each, the Rust on all cores, one thread and three in turn |
| `bc2` | 600,000 blocks unpacked each way and as many packed twice |
| `bc6h_unpack` | 1.5 million blocks, each as signed and as unsigned |
| `bc6h_pack` | 240,000 blocks through each of the five profiles |
| `bc6h_enc` | 60,000 blocks, each as signed and as unsigned; the C++ built by gcc and by clang |
| `miniz` | 30,000 files, 800 megabytes in all, each compressed two or three ways and decompressed seven |
| `stb_image_write` | 9,000 images, 32 million pixels, each as three files; the C built by gcc and by clang at several optimisation levels |
| `stb_image` | 60,000 files, of which 46,500 were read (105 million pixels) and 13,300 refused, each for the same reason; the C built by gcc and by clang at several optimisation levels |

The JPEG reader is the one port with a choice of original to match. stb_image has two
versions of its inverse transform: plain C, and SSE2 or NEON where the processor has them. For
a JPEG file that an encoder wrote they give the same pixels (the two builds of the original
were compared on 20,000 such files). For a damaged file, whose values can be ones no encoder
writes, they can differ, since the SIMD version keeps fewer bits along the way. The port
follows the plain C, which gives one answer on every processor.

`cargo test` needs neither a C++ compiler nor ISPC. Among its tests, ten run made-up blocks,
images and files through the ports and compare their hashes with ones recorded from the
originals: the nine in `tests/*_golden.rs` and one in `src/bc7enc.rs`.

The files have their own check. `compare/files.sh` writes textures of every format and shape
(plain, texture array, cube map, array of cube maps, volume texture) as `.dds`, `.ktx` and
`.ktx2`, the last also with ZLIB supercompression, then has other readers read them: the
Khronos validator has to pass every `.ktx2` file, and the Khronos library ([KTX-Software](https://github.com/KhronosGroup/KTX-Software))
and [tinyddsloader](https://github.com/benikabocha/tinyddsloader) have to find in each file the
same sizes, layers, faces and levels, and the same bytes in every image:

```text
KTX=<checkout of KTX-Software> KTX_BUILD=<its build folder> TINYDDS=<checkout of tinyddsloader> \
    compare/files.sh
```

## Speed

A 512 by 512 image (16,384 blocks) on a two-core x86-64 machine, in thousands of blocks a
second. `cargo run --release --example bench` makes this table on yours.

| | One thread | Two threads |
|---|---|---|
| BC1, level 5 | 331 | 344 |
| BC1, level 18 | 33 | 69 |
| BC3, level 10 | 77 | 133 |
| BC4 | 115 | 184 |
| BC5 | 89 | 156 |
| BC7, `bc7enc`, uber 0 | 134 | 218 |
| BC7, `bc7enc`, uber 4 | 66 | 113 |
| BC7, `bc7e`, `ultrafast` | 193 | 245 |
| BC7, `bc7e`, `basic` | 31 | 54 |
| BC7, `bc7e`, `slowest` | 7.6 | 15 |
| BC7, `bc7e` `basic`, with the rate-distortion pass | 8.6 | 20 |
| BC6H, `veryfast` | 902 | 1,891 |
| BC6H, `basic` | 53 | 118 |
| BC6H, `veryslow` | 6.1 | 11 |

Against the originals on one thread of the same machine: `bc7e` takes about four fifths of the
time of the strict SSE2 build of `bc7e.ispc` and about a fifth longer than its AVX2 fast-math
build; BC6H takes about one and a half times as long as the strict SSE2 build of Intel's
encoder, which packs four blocks at a time. Take all of this as a rough guide: it is one
machine and made-up pixels.

## Limitations

- **No SIMD written by hand.** Stable Rust has no SIMD without `unsafe`, and the crate has no
  `unsafe`. Its speed comes from using every core and from loops the compiler can vectorise by
  itself. On one core, the ISPC originals built for AVX2 are faster.
- **`.ktx2` supercompression: zlib only, so far.** Zstandard is not written or read yet.
  BasisLZ, the third scheme the format has, cannot hold these formats at all: it is the
  packing of Basis Universal's own ETC1S format.
- **Picture files: `.png`, `.bmp` and `.jpg`.** Those are read and written. `.exr`, for HDR
  images, is not yet: use an image crate for that and hand over the floats. Of the other
  formats stb_image reads (`.tga`, `.gif`, `.psd`, `.hdr`, `.pnm`), none is ported.
- **x86's float-to-int rules are built in.** Where an original turns a float that does not fit
  into an int, the ports give what x86 gives, on every processor, so the blocks are the same
  everywhere. The originals themselves would differ on other processors.

## Credits and licence

All of the compression logic is the work of others, and this crate is a translation of it into
Rust:

- `rgbcx`, `bc7enc`, `bc7decomp`, `ert` and `rdo_bc_encoder` are by Richard Geldreich, Jr.,
  from [bc7enc_rdo](https://github.com/richgel999/bc7enc_rdo).
- `bc7e` is by Binomial LLC, Copyright (C) 2018-2020, from the same repository (first released
  as [bc7e](https://github.com/BinomialLLC/bc7e)).
- BC6H unpacking is by Sergii Kudlai, from [bcdec](https://github.com/iOrange/bcdec).
- BC6H packing is by Intel Corporation, Copyright 2017, from the
  [ISPC Texture Compressor](https://github.com/GameTechDev/ISPCTextureCompressor), and for
  `bc6h_enc` by Microsoft (DirectXTex), made into one file by Alexander Samusev as
  [bc6h_enc](https://github.com/0xc0de/bc6h_enc).
- zlib is by Rich Geldreich and others, from [miniz](https://github.com/richgel999/miniz).
- The `.png`, `.bmp` and `.jpg` readers and writers are by Sean Barrett and others, from
  [stb](https://github.com/nothings/stb); the JPEG writer is after Jon Olick's `jo_jpeg.cpp`.

rustbcx is available under the Apache License 2.0, the licence of `bc7e.ispc`: see
[`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). The other originals are under the MIT licence, or
MIT or public domain at your choice; their notices are kept in
[`LICENSE-bc7enc_rdo.txt`](LICENSE-bc7enc_rdo.txt), [`LICENSE-bcdec.txt`](LICENSE-bcdec.txt),
[`LICENSE-ispc-texcomp.txt`](LICENSE-ispc-texcomp.txt), [`LICENSE-bc6h_enc.txt`](LICENSE-bc6h_enc.txt),
[`LICENSE-miniz.txt`](LICENSE-miniz.txt) and [`LICENSE-stb.txt`](LICENSE-stb.txt).

Bug reports and contributions are welcome. If you find a block that this crate packs or
unpacks differently from the originals, please open an issue: that is exactly the kind of bug
this project exists to rule out.
