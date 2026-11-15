//! The crate's half of the check of its files against other readers (`compare/files.sh`).
//!
//!   files <folder>            writes textures of every format and shape as `.dds`, `.ktx`
//!                             and `.ktx2` into the folder (and as `.zlib.ktx2`, a `.ktx2`
//!                             file with ZLIB supercompression), reads each back, and prints
//!                             what a reader should find in each: its size, layers, faces
//!                             and levels, and a hash of every image of every level
//!   files <folder> <ending>   reads the files of that ending that another program made of
//!                             them in the folder, and prints what it finds in each, the
//!                             same way
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
    println!("{name} {}x{}x{} layers {} cube {} mips {}", t.width, t.height, t.depth.max(1), t.layers.max(1), u8::from(t.cube), t.levels.len());
    for level in 0..t.levels.len() {
        for index in 0..t.images_per_level(level) {
            println!("{name} mip {level} image {index} {:016x}", fnv(t.image(level, index).unwrap()));
        }
    }
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: files <folder> [ending]");
        std::process::exit(2);
    };
    let ending = std::env::args().nth(2);
    let formats = [("bc1", TextureFormat::Bc1), ("bc2", TextureFormat::Bc2), ("bc3", TextureFormat::Bc3), ("bc4", TextureFormat::Bc4), ("bc5", TextureFormat::Bc5), ("bc6hu", TextureFormat::Bc6hUnsigned), ("bc6hs", TextureFormat::Bc6hSigned), ("bc7", TextureFormat::Bc7)];
    let shapes = [("plain", 20u32, 12u32, 0u32, 0u32, false), ("array3", 20, 12, 0, 3, false), ("array1", 20, 12, 0, 1, false), ("cube", 16, 16, 0, 0, true), ("cubes2", 16, 16, 0, 2, true), ("volume5", 20, 12, 5, 0, false), ("volume1", 20, 12, 1, 0, false)];
    let mut state = 99u64;
    for (fname, format) in formats {
        for (sname, width, height, depth, layers, cube) in shapes {
            for levels in [1usize, 0] {
                for srgb in [false, true] {
                    if srgb && !format.can_be_srgb() {
                        continue;
                    }
                    let mut t = Texture { format, srgb, width, height, depth, layers, cube, levels: Vec::new() };
                    let count = if levels == 0 { t.max_levels() as usize } else { levels };
                    let name = format!("{fname}-{sname}-{count}{}", if srgb { "-srgb" } else { "" });
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
                            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                            *b = (state >> 40) as u8;
                        }
                        // Textures with all their levels are made of a few blocks over and over,
                        // so that the supercompressed files have something to compress.
                        if levels == 0 {
                            let size = format.bytes_per_block();
                            for i in 8 * size..bytes.len() {
                                if i % size == 0 {
                                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                                }
                                bytes[i] = bytes[((state >> 40) % 8) as usize * size + i % size];
                            }
                        }
                        // BC7 and BC6H blocks have to be of a mode: mode 6 for BC7 and mode 11 for
                        // BC6H.
                        if format == TextureFormat::Bc7 {
                            for block in bytes.chunks_exact_mut(16) {
                                block[0] = (block[0] & 0x80) | 0x40;
                            }
                        }
                        if matches!(format, TextureFormat::Bc6hUnsigned | TextureFormat::Bc6hSigned) {
                            for block in bytes.chunks_exact_mut(16) {
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
                    t.save_ktx2(&file, Supercompression::Zlib(1 + (state >> 50) as i32 % 9)).unwrap();
                    assert_eq!(Texture::load(&file).unwrap(), t, "{file}");
                    // What a reader should find.
                    describe(&name, &t);
                }
            }
        }
    }
}
