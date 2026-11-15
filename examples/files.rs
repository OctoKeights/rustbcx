//! The crate's half of the check of its files against other readers (`compare/files.sh`):
//! writes textures of every format and shape as `.dds`, `.ktx` and `.ktx2` into a folder, reads
//! each back, and prints what a reader should find in each: its size, layers, faces and levels,
//! and a hash of every image of every level.
//!
//!   files <folder>
//!
//! The blocks are made up from a seed; BC6H and BC7 blocks are given a mode, so that a
//! validator that looks into them finds nothing wrong.

use rustbcx::texture::{Texture, TextureFormat};

fn fnv(data: &[u8]) -> u64 {
    let mut h = 14695981039346656037u64;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: files <folder>");
        std::process::exit(2);
    };
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
                    for level in 0..count {
                        let mut bytes = vec![0u8; t.level_bytes(level)];
                        for b in &mut bytes {
                            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                            *b = (state >> 40) as u8;
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
                        let name = format!("{fname}-{sname}-{count}{}.{ext}", if srgb { "-srgb" } else { "" });
                        t.save(format!("{out}/{name}")).unwrap();
                        let mut back = Texture::load(format!("{out}/{name}")).unwrap();
                        // (A .dds file cannot say that one texture is an array.)
                        if ext == "dds" && layers == 1 {
                            back.layers = 1;
                        }
                        assert_eq!(back, t, "{name}");
                    }
                    // What a reader should find.
                    let name = format!("{fname}-{sname}-{count}{}", if srgb { "-srgb" } else { "" });
                    println!("{name} {}x{}x{} layers {} cube {} mips {}", width, height, depth.max(1), layers.max(1), u8::from(cube), count);
                    for level in 0..count {
                        for index in 0..t.images_per_level(level) {
                            println!("{name} mip {level} image {index} {:016x}", fnv(t.image(level, index).unwrap()));
                        }
                    }
                }
            }
        }
    }
}
