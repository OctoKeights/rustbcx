# Examples

[Overview](../README.md) · **Examples** · [Guide](guide.md) · [Behaviour](behaviour.md) · [Verification](verification.md) · [Performance](performance.md) · [Credits](credits.md)

---

Every example on this page is compiled and executed by `cargo test`.

- [Image to a mipmapped texture file](#image-to-a-mipmapped-texture-file)
- [Image files in and out](#image-files-in-and-out)
- [Rate-distortion optimisation](#rate-distortion-optimisation)
- [HDR images and BC6H](#hdr-images-and-bc6h)
- [Cube maps, arrays and volumes](#cube-maps-arrays-and-volumes)
- [Individual blocks](#individual-blocks)

## Image to a mipmapped texture file

Compress an image to a `.dds` file with a full mip chain, then read it back:

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

## Image files in and out

Load a `.png`, `.bmp` or `.jpg` file, compress it, and save the decoded result for inspection:

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

## Rate-distortion optimisation

Enable the rate-distortion pass to produce a file that compresses further on disk:

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

## HDR images and BC6H

Compress an HDR image to BC6H and decode it:

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

`.exr` files follow the same pattern: `HdrImage::load("sky.exr")` reads the red, green and
blue channels, whether stored as half or full floats, and `save` writes one.

For negative values, use signed BC6H: `HdrImage::from_rgb_f32_signed` (or `load_signed`)
preserves them, and `texture::compress_hdr_signed` encodes them with the exhaustive encoder
in `bc6h_enc`.

## Cube maps, arrays and volumes

Compress six images to a cube map. Texture arrays and volume textures follow the same
pattern:

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

## Individual blocks

Encode and decode block by block when the surrounding pipeline is your own. A block is
sixteen pixels, ordered left to right and top to bottom:

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
