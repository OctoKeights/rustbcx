//! The crate's half of the check of its files against other readers (`compare/files.sh`).
//!
//!   files <folder>            writes textures of every format and shape as `.dds`, `.ktx`
//!                             and `.ktx2` into the folder (and as `.zlib.ktx2` and
//!                             `.zstd.ktx2`, `.ktx2` files with ZLIB and with Zstandard
//!                             supercompression), reads each back, and prints
//!                             what a reader should find in each: its size, layers, faces
//!                             and levels, and a hash of every image of every level
//!   files <folder> <ending>   reads the files of that ending that another program made of
//!                             them in the folder, and prints what it finds in each, the
//!                             same way
//!
//!   files --exr <folder>      writes made-up images as `.exr` files into the folder: in
//!                             scanlines, in scanlines of an image of six pixels, and in
//!                             tiles; as halves and as floats; with each of the eight
//!                             compressions. Beside each goes what the crate reads of it, as
//!                             the floats another reader should find: `<name>.exr.gbr`, the
//!                             greens, then the blues, then the reds
//!   files --exr-same <file.exr> <file.gbr>
//!                             reads a `.exr` file that another program wrote, and fails
//!                             unless its floats are those of the other file
//!
//! The blocks are made up from a seed; BC6H and BC7 blocks are given a mode, so that a
//! validator that looks into them finds nothing wrong.

use rustbcx::ktx::Supercompression;
use rustbcx::texture::{Texture, TextureFormat};

fn fnv(data: &[u8]) -> u64 {
    let mut h = 14695981039346656037u64;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Prints what a reader should find in a texture's file.
fn describe(name: &str, t: &Texture) {
    println!(
        "{name} {}x{}x{} layers {} cube {} mips {}",
        t.width,
        t.height,
        t.depth.max(1),
        t.layers.max(1),
        u8::from(t.cube),
        t.levels.len()
    );
    for level in 0..t.levels.len() {
        for index in 0..t.images_per_level(level) {
            println!(
                "{name} mip {level} image {index} {:016x}",
                fnv(t.image(level, index).unwrap())
            );
        }
    }
}

/// The `.exr` files, and beside each what the crate reads of it.
fn exr(out: &str) {
    use rustbcx::tinyexr::{self, Box2i, ChannelData, ChannelInfo, ExrHeader, ExrImage, ExrTile};
    let compressions = [
        ("none", tinyexr::COMPRESSIONTYPE_NONE),
        ("rle", tinyexr::COMPRESSIONTYPE_RLE),
        ("zips", tinyexr::COMPRESSIONTYPE_ZIPS),
        ("zip", tinyexr::COMPRESSIONTYPE_ZIP),
        ("piz", tinyexr::COMPRESSIONTYPE_PIZ),
        ("pxr24", tinyexr::COMPRESSIONTYPE_PXR24),
        ("b44", tinyexr::COMPRESSIONTYPE_B44),
        ("b44a", tinyexr::COMPRESSIONTYPE_B44A),
    ];
    for (shape, w, h, tile) in [
        ("lines", 37i32, 45i32, 0i32),
        ("tiny", 3, 2, 0),
        ("tiles", 37, 45, 16),
    ] {
        // Red, green and blue of each pixel: a few values over and over, and a little on top.
        let values: Vec<f32> = (0..(w * h * 3) as usize)
            .map(|i| ((i * 7919) % 1000) as f32 / 250.0 + (i % 7) as f32 * 0.001)
            .collect();
        for (compression, compression_type) in compressions {
            for (stored, file_type) in [
                ("half", tinyexr::PIXELTYPE_HALF),
                ("float", tinyexr::PIXELTYPE_FLOAT),
            ] {
                let mut header = ExrHeader {
                    channels: ["B", "G", "R"]
                        .iter()
                        .map(|name| ChannelInfo {
                            name: name.to_string(),
                            pixel_type: file_type,
                            x_sampling: 1,
                            y_sampling: 1,
                            p_linear: 0,
                        })
                        .collect(),
                    pixel_types: vec![tinyexr::PIXELTYPE_FLOAT; 3],
                    requested_pixel_types: vec![file_type; 3],
                    compression_type,
                    data_window: Box2i {
                        min_x: 0,
                        min_y: 0,
                        max_x: w - 1,
                        max_y: h - 1,
                    },
                    ..ExrHeader::default()
                };
                let mut image = ExrImage {
                    width: w,
                    height: h,
                    num_channels: 3,
                    ..ExrImage::default()
                };
                if tile == 0 {
                    let channel = |k: usize| {
                        ChannelData::Float(
                            (0..(w * h) as usize).map(|i| values[i * 3 + k]).collect(),
                        )
                    };
                    image.images = vec![channel(2), channel(1), channel(0)];
                } else {
                    header.tiled = 1;
                    header.tile_size_x = tile;
                    header.tile_size_y = tile;
                    let level = tinyexr::tile_levels(&header).expect("levels")[0];
                    for t in 0..level.num_x_tiles * level.num_y_tiles {
                        let (offset_x, offset_y) = (t % level.num_x_tiles, t / level.num_x_tiles);
                        let (width, height) =
                            (tile.min(w - offset_x * tile), tile.min(h - offset_y * tile));
                        let channel = |k: usize| {
                            let mut tile_values = vec![0f32; (tile * tile) as usize];
                            for y in 0..height {
                                for x in 0..width {
                                    tile_values[(y * tile + x) as usize] =
                                        values[(((offset_y * tile + y) * w + offset_x * tile + x)
                                            * 3)
                                            as usize
                                            + k];
                                }
                            }
                            ChannelData::Float(tile_values)
                        };
                        image.tiles.push(ExrTile {
                            offset_x,
                            offset_y,
                            level_x: 0,
                            level_y: 0,
                            width,
                            height,
                            images: vec![channel(2), channel(1), channel(0)],
                        });
                    }
                }
                let name = format!("{out}/{shape}-{compression}-{stored}.exr");
                let file =
                    tinyexr::save_exr_image_to_memory(&image, &header).expect("an .exr file");
                let (rgba, read_width, read_height) =
                    tinyexr::load_exr_from_memory(&file).expect("the file read back");
                assert_eq!((read_width, read_height), (w, h), "{name}");
                // Floats come back as they were, but from PXR24, which keeps 24 bits of them.
                if file_type == tinyexr::PIXELTYPE_FLOAT
                    && compression_type != tinyexr::COMPRESSIONTYPE_PXR24
                {
                    for (pixel, read) in rgba.as_chunks::<4>().0.iter().enumerate() {
                        assert_eq!(read[..3], values[pixel * 3..pixel * 3 + 3], "{name}");
                    }
                }
                let plane = |k: usize| {
                    rgba.as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(move |pixel| pixel[k].to_le_bytes())
                };
                let planes: Vec<u8> = plane(1).chain(plane(2)).chain(plane(0)).collect();
                std::fs::write(&name, file).expect("write");
                std::fs::write(format!("{name}.gbr"), planes).expect("write");
            }
        }
    }
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: files <folder> [ending]\n       files --exr <folder>\n       files --exr-same <file.exr> <file.gbr>");
        std::process::exit(2);
    };
    if out == "--exr-same" {
        let (Some(file), Some(planes)) = (std::env::args().nth(2), std::env::args().nth(3)) else {
            eprintln!("usage: files --exr-same <file.exr> <file.gbr>");
            std::process::exit(2);
        };
        let (rgba, _, _) =
            match rustbcx::tinyexr::load_exr_from_memory(&std::fs::read(&file).expect("read")) {
                Ok(read) => read,
                Err(error) => {
                    eprintln!("{file}: {error}");
                    std::process::exit(1);
                }
            };
        let plane = |k: usize| {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(move |pixel| pixel[k].to_le_bytes())
        };
        let read: Vec<u8> = plane(1).chain(plane(2)).chain(plane(0)).collect();
        std::process::exit(i32::from(read != std::fs::read(planes).expect("read")));
    }
    if out == "--exr" {
        let Some(out) = std::env::args().nth(2) else {
            eprintln!("usage: files --exr <folder>");
            std::process::exit(2);
        };
        exr(&out);
        return;
    }
    let ending = std::env::args().nth(2);
    let formats = [
        ("bc1", TextureFormat::Bc1),
        ("bc2", TextureFormat::Bc2),
        ("bc3", TextureFormat::Bc3),
        ("bc4", TextureFormat::Bc4),
        ("bc5", TextureFormat::Bc5),
        ("bc6hu", TextureFormat::Bc6hUnsigned),
        ("bc6hs", TextureFormat::Bc6hSigned),
        ("bc7", TextureFormat::Bc7),
    ];
    let shapes = [
        ("plain", 20u32, 12u32, 0u32, 0u32, false),
        ("array3", 20, 12, 0, 3, false),
        ("array1", 20, 12, 0, 1, false),
        ("cube", 16, 16, 0, 0, true),
        ("cubes2", 16, 16, 0, 2, true),
        ("volume5", 20, 12, 5, 0, false),
        ("volume1", 20, 12, 1, 0, false),
    ];
    let mut state = 99u64;
    for (fname, format) in formats {
        for (sname, width, height, depth, layers, cube) in shapes {
            for levels in [1usize, 0] {
                for srgb in [false, true] {
                    if srgb && !format.can_be_srgb() {
                        continue;
                    }
                    let mut t = Texture {
                        format,
                        srgb,
                        width,
                        height,
                        depth,
                        layers,
                        cube,
                        levels: Vec::new(),
                    };
                    let count = if levels == 0 {
                        t.max_levels() as usize
                    } else {
                        levels
                    };
                    let name =
                        format!("{fname}-{sname}-{count}{}", if srgb { "-srgb" } else { "" });
                    if let Some(ending) = &ending {
                        // What another program made of the file, as this crate reads it.
                        match Texture::load(format!("{out}/{name}.{ending}")) {
                            Ok(read) => describe(&name, &read),
                            Err(error) => println!("{name} not read: {error}"),
                        }
                        continue;
                    }
                    for level in 0..count {
                        let mut bytes = vec![0u8; t.level_bytes(level)];
                        for b in &mut bytes {
                            state = state
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            *b = (state >> 40) as u8;
                        }
                        // Textures with all their levels are made of a few blocks over and over,
                        // so that the supercompressed files have something to compress.
                        if levels == 0 {
                            let size = format.bytes_per_block();
                            for i in 8 * size..bytes.len() {
                                if i % size == 0 {
                                    state = state
                                        .wrapping_mul(6364136223846793005)
                                        .wrapping_add(1442695040888963407);
                                }
                                bytes[i] = bytes[((state >> 40) % 8) as usize * size + i % size];
                            }
                        }
                        // BC7 and BC6H blocks have to be of a mode: mode 6 for BC7 and mode 11 for
                        // BC6H.
                        if format == TextureFormat::Bc7 {
                            for block in bytes.as_chunks_mut::<16>().0.iter_mut() {
                                block[0] = (block[0] & 0x80) | 0x40;
                            }
                        }
                        if matches!(
                            format,
                            TextureFormat::Bc6hUnsigned | TextureFormat::Bc6hSigned
                        ) {
                            for block in bytes.as_chunks_mut::<16>().0.iter_mut() {
                                block[0] = (block[0] & 0xe0) | 0x03;
                            }
                        }
                        t.levels.push(bytes);
                    }
                    assert!(t.is_valid());
                    for ext in ["dds", "ktx", "ktx2"] {
                        let file = format!("{out}/{name}.{ext}");
                        t.save(&file).unwrap();
                        let mut back = Texture::load(&file).unwrap();
                        // (A .dds file cannot say that one texture is an array.)
                        if ext == "dds" && layers == 1 {
                            back.layers = 1;
                        }
                        assert_eq!(back, t, "{file}");
                    }
                    let file = format!("{out}/{name}.zlib.ktx2");
                    t.save_ktx2(&file, Supercompression::Zlib(1 + (state >> 50) as i32 % 9))
                        .unwrap();
                    assert_eq!(Texture::load(&file).unwrap(), t, "{file}");
                    let file = format!("{out}/{name}.zstd.ktx2");
                    t.save_ktx2(
                        &file,
                        Supercompression::Zstd([1, 2, 3, 0, -1, -7][(state >> 45) as usize % 6]),
                    )
                    .unwrap();
                    assert_eq!(Texture::load(&file).unwrap(), t, "{file}");
                    // What a reader should find.
                    describe(&name, &t);
                }
            }
        }
    }
}
