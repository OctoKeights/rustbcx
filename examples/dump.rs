//! The Rust half of the comparison with bc7enc_rdo: runs the port over made-up blocks and
//! prints a running hash of what comes out, as `compare/dump.cpp` does with the C++ library.
//! `compare/compare.sh` compares the two.
//!
//!   dump bc7decomp <seed> <blocks>   BC7 blocks of every mode (and of none) unpacked
//!   dump unpack <seed> <blocks>      BC1, BC3, BC4 and BC5 blocks unpacked every way, and solid
//!                                    BC1 blocks made
//!   dump bc1 <seed> <blocks>         made-up pixels packed as BC1 at every level, with every
//!                                    flag, for each way of interpolating, and unpacked again
//!   dump bc345 <seed> <blocks>       the same pixels packed as BC3, BC4 and BC5, each way
//!   dump bc7enc <seed> <blocks>      the same pixels packed as BC7 with every setting varied,
//!                                    and unpacked again
//!   dump bc7block <seed> <blocks>    BC7 blocks of every mode written from made-up endpoints
//!                                    and selectors, and unpacked again
//!   dump rdo <seed> <blocks>         made-up images of every shape packed whole by the image
//!                                    encoder in each of its formats, with and without the
//!                                    rate-distortion pass, every setting varied; BC7 with
//!                                    bc7enc
//!   dump rdo_bc7e <seed> <blocks>    the same, BC7 with bc7e for about half the images (the
//!                                    C++ side needs the ISPC object for it)
//!   dump ert <seed> <blocks>         runs of made-up blocks, many of them near copies of the
//!                                    blocks before, packed in one of five formats and put
//!                                    through the entropy reduction transform, its settings
//!                                    varied
//!
//!   dump bc2 <seed> <blocks>         made-up BC2 blocks unpacked each way of interpolating,
//!                                    and the same pixels packed as BC2 (the C++ side needs
//!                                    bcdec for it)
//!
//! And of the comparison for BC6H, as `compare/dump_bc6h.cpp` does with bcdec:
//!
//!   dump bc6h_unpack <seed> <blocks>   made-up BC6H blocks of every mode (and of none)
//!                                      unpacked as unsigned and as signed, to halves and to
//!                                      floats
//!   dump bc6h_pack <seed> <blocks>     made-up HDR pixels packed with each of the packer's
//!                                      five profiles, and unpacked again, as
//!                                      `compare/dump_bc6h.cpp` does with Intel's ISPC Texture
//!                                      Compressor
//!   dump bc6h_enc <seed> <blocks>      made-up HDR pixels of both signs packed as signed and as
//!                                      unsigned BC6H by the port of bc6h_enc, as
//!                                      `compare/dump_bc6h_enc.cpp` does with the original
//!
//! And of the comparison with miniz, as `compare/dump_miniz.c` does:
//!
//!   dump miniz <seed> <count>   made-up data compressed with flags from every level and
//!                               strategy and with made-up flags, in one go and in pieces;
//!                               each stream decompressed whole, into too little room,
//!                               damaged, cut short and with bytes after it; and made-up
//!                               bytes decompressed
//!
//! And of the comparison with stb_image_write.h, as `compare/dump_stb_image_write.c` does:
//!
//!   dump stb_image_write <seed> <count>   made-up images written as .png, .bmp and .jpg with
//!                                         made-up settings, and made-up data through the
//!                                         file's own zlib compressor
//!
//! And of the comparison with stb_image.h, as `compare/dump_stb_image.c` does:
//!
//!   dump stb_image <seed> <count>   made-up .png, .bmp and .jpg files, a third of them
//!                                   damaged, asked their size and read at 8 and 16 bits a
//!                                   channel; and made-up data through its zlib decoder
//!   dump stb_image_more <seed> <count>
//!                                   made-up .tga, .psd, .gif, .pnm and .hdr files, a third of
//!                                   them damaged, asked their size, read at 8 and 16 bits a
//!                                   channel and as floats, and as animations
//!
//! And of the comparison with zstd's reference decoder, as `compare/dump_zstd.c` does:
//!
//!   dump zstd_decode <seed> <count> <file>         the frames that program wrote to the file,
//!   dump zstd_decode_small <seed> <count> <file>   each asked its size and decoded
//!
//! And of the comparison with zstd's compressor, as `compare/dump_zstd_compress.c` does:
//!
//!   dump zstd_compress <seed> <count>              made-up data compressed at levels 1 to 4,
//!   dump zstd_compress_small <seed> <count>        0 and the negative ones
//!   dump zstd_compress_more <seed> <count>         the same at levels 4 to 22
//!   dump zstd_compress_more_small <seed> <count>
//!   dump zstd_compress_long <seed> <count>         64 to 80 megabytes at level 22
//!
//! And of the comparison with tinyexr, as `compare/dump_tinyexr.cc` does:
//!
//!   dump tinyexr_write <seed> <count>               made-up images saved as .exr files, and
//!   dump tinyexr_write_small <seed> <count>         every fourth the short way as well
//!   dump tinyexr_read <seed> <count> <file>         the .exr files that program wrote to the
//!   dump tinyexr_read_small <seed> <count> <file>   file, each read four ways
//!   dump tinyexr_parts <seed> <count> <file>        files of many parts saved, and the
//!                                                   ones that program wrote to the file read
//!   dump tinyexr_deep <seed> <count> <file>         the deep files that program wrote, read
//!
//! And of the comparison with bc7e.ispc, as `compare/dump_bc7e.cpp` does with the ISPC encoder:
//!
//!   dump bc7e_presets <seed> <blocks> [file]    made-up blocks packed with each of the seven
//!                                               profiles, for both error measures
//!   dump bc7e_settings <seed> <blocks> [file]   the blocks in groups of 32, each group with
//!                                               settings made up from the seed
//!
//! With a file named, every block bc7e packed is written to it as well.

// (The arithmetic is written as dump.cpp has it.)
#![allow(clippy::manual_is_multiple_of, clippy::needless_range_loop)]

use rustbcx::bc7decomp;
use rustbcx::bc7e::Bc7e;
use rustbcx::bc7enc::{self, Bc7Enc, CompressBlockParams, OptimizationResults};
use rustbcx::rgbcx::{self, Bc1ApproxMode, Rgbcx};
use std::io::Write;

/// The made-up blocks, and the dump the tests run as well (`tests/ert_golden.rs`).
#[path = "../tests/made_up/mod.rs"]
mod made_up;

use made_up::{make_pixels, Hash, Random};

/// The blocks and settings the bc7e dumps pack, shared with `tests/bc7e_golden.rs`.
#[path = "../tests/bc7e_made_up/mod.rs"]
mod bc7e_made_up;

fn dump_bc7decomp(seed: u64, blocks: u32) {
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut unpacked = [0u32; 9];
    for i in 0..blocks {
        let mut block = [0u8; 16];
        for byte in &mut block {
            *byte = random.next() as u8;
        }
        // Every third block is of a mode chosen in turn (8: of none); the rest are as they fell.
        if i % 3 == 0 {
            let mode = (i / 3) % 9;
            block[0] = if mode == 8 { 0 } else { ((u32::from(block[0]) & !((2u32 << mode) - 1)) | (1 << mode)) as u8 };
        }
        let mut pixels = [[0xEEu8; 4]; 16];
        let ok = bc7decomp::unpack_bc7(&block, &mut pixels);
        let mode = (block[0].trailing_zeros() as usize).min(8);
        unpacked[mode] += u32::from(ok);
        hash.add(&[u8::from(ok)]);
        hash.add(pixels.as_flattened());
        if (i + 1) % 100000 == 0 || i + 1 == blocks {
            println!("bc7decomp {} {:016x}", i + 1, hash.value);
        }
    }
    print!("bc7decomp unpacked by mode:");
    for count in unpacked {
        print!(" {count}");
    }
    println!();
}

const MODES: [Bc1ApproxMode; 4] = [Bc1ApproxMode::Ideal, Bc1ApproxMode::NVidia, Bc1ApproxMode::Amd, Bc1ApproxMode::IdealRound4];

fn halves(block: &[u8; 16]) -> (&[u8; 8], &[u8; 8]) {
    let (first, second) = block.split_at(8);
    (first.try_into().unwrap(), second.try_into().unwrap())
}

fn dump_unpack(seed: u64, blocks: u32) {
    let encoders = MODES.map(Rgbcx::new);
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut punchthrough = 0u32;
    for i in 0..blocks {
        let mut block = [0u8; 16];
        for byte in &mut block {
            *byte = random.next() as u8;
        }
        let mode = MODES[(i % 4) as usize];
        let (first, second) = halves(&block);

        let mut colors = [[0u8; 4]; 4];
        let flag = rgbcx::unpack_bc1_block_colors(first, &mut colors, mode);
        hash.add(&[u8::from(flag)]);
        hash.add(colors.as_flattened());

        let mut pixels = [[0x77u8; 4]; 16];
        let flag = rgbcx::unpack_bc1(first, &mut pixels, true, mode);
        punchthrough += u32::from(flag);
        hash.add(&[u8::from(flag)]);
        hash.add(pixels.as_flattened());

        let mut pixels = [[0x77u8; 4]; 16];
        let flag = rgbcx::unpack_bc1(second, &mut pixels, false, mode);
        hash.add(&[u8::from(flag)]);
        hash.add(pixels.as_flattened());

        let mut pixels = [[0x77u8; 4]; 16];
        let flag = rgbcx::unpack_bc3(&block, &mut pixels, mode);
        hash.add(&[u8::from(flag)]);
        hash.add(pixels.as_flattened());

        let mut bytes = [0x77u8; 64];
        rgbcx::unpack_bc4(first, &mut bytes, 1);
        rgbcx::unpack_bc4(second, &mut bytes[16..], 3);
        hash.add(&bytes);

        let mut bytes = [0x77u8; 64];
        rgbcx::unpack_bc5(&block, &mut bytes, 0, 1, 4);
        rgbcx::unpack_bc5(&block, &mut bytes, 3, 2, 4);
        hash.add(&bytes);

        // A solid block of a made-up colour, and of one whose channels are close together.
        let encoder = &encoders[((i / 4) % 4) as usize];
        let mut solid = [0u8; 8];
        let (r, g, b) = (random.next() % 256, random.next() % 256, random.next() % 256);
        encoder.encode_bc1_solid_block(&mut solid, r, g, b, i % 2 == 0);
        hash.add(&solid);
        let (g, b) = ((r + random.next() % 3) % 256, (r + random.next() % 3) % 256);
        encoder.encode_bc1_solid_block(&mut solid, r, g, b, i % 2 == 1);
        hash.add(&solid);

        if (i + 1) % 100000 == 0 || i + 1 == blocks {
            println!("unpack {} {:016x}", i + 1, hash.value);
        }
    }
    println!("unpack punchthrough: {punchthrough}");
}

fn dump_bc1(seed: u64, blocks: u32) {
    let encoders = MODES.map(Rgbcx::new);
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut three_color = 0u32;
    for i in 0..blocks {
        // The four ways of interpolating take turns, a hundred blocks each.
        let mode = MODES[((i / 100) % 4) as usize];
        let encoder = &encoders[((i / 100) % 4) as usize];

        let pixels = make_pixels(&mut random);

        let mut forced = [0u8; 16];
        for value in &mut forced {
            *value = (random.next() % 4) as u8;
        }
        let options = random.next();
        let force = if options & 15 == 0 { Some(&forced) } else { None };

        let mut block = [0xEEu8; 8];
        if i % 3 != 2 {
            // By level: every level in turn, the slowest (19) once in 64 of its turns.
            let mut level = i % 20;
            if level == 19 && (i / 20) % 64 != 0 {
                level = 5 + (i / 20) % 3;
            }
            encoder.encode_bc1(level, &mut block, &pixels, options & 16 != 0, options & 32 != 0, force);
        } else {
            // By flags: any of them, with made-up numbers of orderings and rounds.
            const FLAGS: [u32; 13] = [
                rgbcx::ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS,
                rgbcx::ENCODE_BC1_TWO_LEAST_SQUARES_PASSES,
                rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS,
                rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS,
                rgbcx::ENCODE_BC1_ITERATIVE,
                rgbcx::ENCODE_BC1_BOUNDING_BOX,
                rgbcx::ENCODE_BC1_USE_FASTER_MSE_EVAL,
                rgbcx::ENCODE_BC1_USE_FULL_MSE_EVAL,
                rgbcx::ENCODE_BC1_USE_2D_LS,
                rgbcx::ENCODE_BC1_USE_6_POWER_ITERS,
                rgbcx::ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS,
                rgbcx::ENCODE_BC1_BOUNDING_BOX_INT,
                0,
            ];
            let mut flags = 0u32;
            let bits = random.next();
            for (k, flag) in FLAGS.iter().enumerate() {
                if bits & (1 << k) != 0 {
                    flags |= flag;
                }
            }
            // (Without a need for the block's error the encoder takes one first guess only.)
            let needs_error = flags & (rgbcx::ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS | rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS | rgbcx::ENCODE_BC1_USE_FULL_MSE_EVAL) != 0;
            if bits & (1 << 13) != 0 && needs_error {
                flags |= (random.next() % 40) << rgbcx::ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_SHIFT;
            }
            if !needs_error && flags & rgbcx::ENCODE_BC1_ENDPOINT_SEARCH_ROUNDS_MASK == 0 {
                flags &= !rgbcx::ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS;
            }
            let (orderings4, orderings3) = (random.next() % 140, random.next() % 40);
            encoder.encode_bc1_with_flags(&mut block, &pixels, flags, orderings4, orderings3, force);
        }
        hash.add(&block);

        let mut unpacked = [[0u8; 4]; 16];
        let flag = rgbcx::unpack_bc1(&block, &mut unpacked, true, mode);
        three_color += u32::from(flag);
        hash.add(&[u8::from(flag)]);
        hash.add(unpacked.as_flattened());

        if (i + 1) % 20000 == 0 || i + 1 == blocks {
            println!("bc1 {} {:016x}", i + 1, hash.value);
        }
    }
    println!("bc1 blocks of three colours: {three_color}");
}

fn dump_bc345(seed: u64, blocks: u32) {
    let encoders = MODES.map(Rgbcx::new);
    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut error = 0u64;
    for i in 0..blocks {
        let mode = MODES[((i / 100) % 4) as usize];
        let encoder = &encoders[((i / 100) % 4) as usize];

        let pixels = make_pixels(&mut random);
        let flat = pixels.as_flattened();
        let options = random.next();
        let level = options % 13;
        let radius = (options >> 8) % 6;
        let modes = 1 + (options >> 12) % 3;
        let mut forced = [0u8; 16];
        for value in &mut forced {
            *value = (random.next() % 8) as u8;
        }

        let mut block = [0xEEu8; 16];
        let mut unpacked = [[0u8; 4]; 16];
        encoder.encode_bc3(level, &mut block, &pixels);
        hash.add(&block);
        let flag = rgbcx::unpack_bc3(&block, &mut unpacked, mode);
        hash.add(&[u8::from(flag)]);
        hash.add(unpacked.as_flattened());

        // (Without a need for the block's error the encoder takes one first guess only.)
        let mut flags = options >> 16;
        if flags & (rgbcx::ENCODE_BC1_USE_LIKELY_TOTAL_ORDERINGS | rgbcx::ENCODE_BC1_USE_FULL_MSE_EVAL) == 0 {
            flags &= !rgbcx::ENCODE_BC1_TRY_ALL_INITIAL_ENDPOINTS;
        }
        let mut block = [0xEEu8; 16];
        encoder.encode_bc3_with_flags(&mut block, &pixels, flags, 1 + (options >> 4) % 12);
        hash.add(&block);

        let mut block = [0xEEu8; 16];
        encoder.encode_bc3_hq(level, &mut block, &pixels, radius, modes);
        hash.add(&block);

        let mut block = [0xEEu8; 16];
        let channel = (i % 4) as usize;
        {
            let (first, second) = block.split_at_mut(8);
            rgbcx::encode_bc4(first.try_into().unwrap(), &flat[channel..], 4);
            error += u64::from(rgbcx::encode_bc4_hq(second.try_into().unwrap(), &flat[channel..], 4, radius, modes, if options & 0x80 != 0 { Some(&forced) } else { None }));
        }
        hash.add(&block);
        let mut bytes = [0u8; 32];
        let (first, second) = halves(&block);
        rgbcx::unpack_bc4(first, &mut bytes, 1);
        rgbcx::unpack_bc4(second, &mut bytes[16..], 1);
        hash.add(&bytes);

        let mut block = [0xEEu8; 16];
        rgbcx::encode_bc5(&mut block, flat, 0, 1, 4);
        hash.add(&block);
        rgbcx::encode_bc5(&mut block, flat, 2, 3, 4);
        hash.add(&block);
        rgbcx::encode_bc5_hq(&mut block, flat, 1, 2, 4, radius, modes);
        hash.add(&block);
        let mut bytes = [0x77u8; 64];
        rgbcx::unpack_bc5(&block, &mut bytes, 1, 2, 4);
        hash.add(&bytes);

        if (i + 1) % 20000 == 0 || i + 1 == blocks {
            println!("bc345 {} {:016x}", i + 1, hash.value);
        }
    }
    println!("bc345 error of the searched BC4 blocks: {error}");
}

fn bc7_mode(block: &[u8; 16]) -> usize {
    (block[0].trailing_zeros() as usize).min(8)
}

fn dump_bc7enc(seed: u64, blocks: u32) {
    const PARTITIONS: [u32; 8] = [64, 64, 64, 32, 16, 1, 0, 100];
    const MASKS: [u32; 8] = [u32::MAX, u32::MAX, u32::MAX, u32::MAX, 1 << 6, (1 << 1) | (1 << 5), (1 << 1) | (1 << 7), (1 << 1) | (1 << 5) | (1 << 7)];
    const PBIT_WEIGHTS: [f32; 4] = [1.0, 1.0, 1.3, 0.75];
    const ERROR_WEIGHTS: [f32; 4] = [1.0, 1.0, 1.5, 0.5];
    const PARTITION_WEIGHTS: [f32; 4] = [1.0, 1.0, 0.8, 1.25];

    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let encoder = Bc7Enc::new();
    let mut packed = [0u32; 9];
    let mut with_alpha = 0u32;
    for i in 0..blocks {
        let mut pixels = make_pixels(&mut random);
        let o1 = random.next();
        let o2 = random.next();
        let mut weights = [0u32; 4];
        for weight in &mut weights {
            *weight = 1 + random.next() % 255;
        }
        let mut selectors = [0u8; 16];
        for selector in &mut selectors {
            *selector = random.next() as u8;
        }

        let mut opaque = o1 % 2 == 0;

        let mut params = CompressBlockParams::new();
        params.uber_level = (o1 >> 4) % 5;
        if (o1 >> 8) & 1 == 0 {
            params.init_linear_weights();
        }
        if (o1 >> 9) % 4 == 0 {
            params.weights = weights;
        }
        params.try_least_squares = (o1 >> 11) % 4 != 0;
        params.mode17_partition_estimation_filterbank = (o1 >> 13) & 1 != 0;
        params.max_partitions = PARTITIONS[((o1 >> 14) % 8) as usize];
        params.force_alpha = (o1 >> 17) % 8 == 0;
        params.quant_mode6_endpoints = (o1 >> 20) % 4 == 0;
        params.bias_mode1_pbits = (o1 >> 22) % 4 == 0;
        params.mode_mask = MASKS[((o1 >> 24) % 8) as usize];
        params.pbit1_weight = PBIT_WEIGHTS[((o2 >> 8) % 4) as usize];
        params.mode1_error_weight = ERROR_WEIGHTS[((o2 >> 10) % 4) as usize];
        params.mode5_error_weight = ERROR_WEIGHTS[((o2 >> 12) % 4) as usize];
        params.mode6_error_weight = ERROR_WEIGHTS[((o2 >> 14) % 4) as usize];
        params.mode7_error_weight = ERROR_WEIGHTS[((o2 >> 16) % 4) as usize];
        params.low_frequency_partition_weight = PARTITION_WEIGHTS[((o2 >> 18) % 4) as usize];
        if o2 % 8 == 0 {
            // The selectors have to fit the modes they may be used with.
            params.force_selectors = true;
            let how = (o2 >> 3) % 3;
            if how == 1 {
                params.mode_mask = 1 << 6;
            } else if how == 2 {
                params.mode_mask = 1 << 1;
                params.force_alpha = false;
                opaque = true;
            }
            for k in 0..16 {
                params.selectors[k] = selectors[k] % (if how == 0 { 4 } else if how == 1 { 16 } else { 8 });
            }
        }
        // (Mode 1 alone needs a partition to try.)
        if params.mode_mask & (1 << 6) == 0 && params.max_partitions == 0 {
            params.max_partitions = 64;
        }
        if opaque {
            for pixel in &mut pixels {
                pixel[3] = 255;
            }
        }

        let mut block = [0xEEu8; 16];
        let flag = encoder.compress_block(&mut block, &pixels, &params);
        with_alpha += u32::from(flag);
        hash.add(&block);
        hash.add(&[u8::from(flag)]);
        packed[bc7_mode(&block)] += 1;

        let mut unpacked = [[0xEEu8; 4]; 16];
        let ok = bc7decomp::unpack_bc7(&block, &mut unpacked);
        hash.add(&[u8::from(ok)]);
        hash.add(unpacked.as_flattened());

        if (i + 1) % 20000 == 0 || i + 1 == blocks {
            println!("bc7enc {} {:016x}", i + 1, hash.value);
        }
    }
    print!("bc7enc blocks with alpha: {with_alpha}, packed by mode:");
    for count in packed {
        print!(" {count}");
    }
    println!();
}

fn dump_bc7block(seed: u64, blocks: u32) {
    const PARTITION_BITS: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
    const COLOR_INDEX_BITS: [u32; 8] = [3, 3, 2, 2, 2, 2, 4, 2];
    const ALPHA_INDEX_BITS: [u32; 8] = [0, 0, 0, 0, 3, 2, 4, 2];
    const COLOR_BITS: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
    const ALPHA_BITS: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];

    let mut random = Random { state: seed };
    let mut hash = Hash::new();
    let mut unpacked_ok = 0u32;
    for i in 0..blocks {
        let mut results = OptimizationResults::default();
        let mode = (i % 8) as usize;
        results.mode = mode as u32;
        results.partition = random.next() % (1 << PARTITION_BITS[mode]);
        results.rotation = if mode == 4 || mode == 5 { random.next() % 4 } else { 0 };
        results.index_selector = if mode == 4 { random.next() % 2 } else { 0 };
        let color_index_bits = COLOR_INDEX_BITS[mode] + results.index_selector;
        let alpha_index_bits = ALPHA_INDEX_BITS[mode] - results.index_selector;
        for k in 0..16 {
            results.selectors[k] = (random.next() % (1 << color_index_bits)) as u8;
            results.alpha_selectors[k] = if mode == 4 || mode == 5 { (random.next() % (1 << alpha_index_bits)) as u8 } else { 0 };
        }
        for k in 0..3 {
            for c in 0..4 {
                let bits = if c == 3 { ALPHA_BITS[mode] } else { COLOR_BITS[mode] };
                results.low[k][c] = (random.next() % (1 << bits)) as u8;
                results.high[k][c] = (random.next() % (1 << bits)) as u8;
            }
            results.pbits[k][0] = random.next() % 2;
            results.pbits[k][1] = random.next() % 2;
        }

        let mut block = [0xEEu8; 16];
        bc7enc::encode_bc7_block(&mut block, &results);
        hash.add(&block);

        let mut unpacked = [[0xEEu8; 4]; 16];
        let ok = bc7decomp::unpack_bc7(&block, &mut unpacked);
        unpacked_ok += u32::from(ok);
        hash.add(&[u8::from(ok)]);
        hash.add(unpacked.as_flattened());

        if (i + 1) % 100000 == 0 || i + 1 == blocks {
            println!("bc7block {} {:016x}", i + 1, hash.value);
        }
    }
    println!("bc7block unpacked: {unpacked_ok}");
}

/// `bc7e_presets` (true) or `bc7e_settings`.
fn dump_bc7e(presets: bool, seed: u64, blocks: u32, path: Option<&String>) {
    use bc7e_made_up::{hash, make_blocks, make_settings, PROFILES};
    let mut random = bc7e_made_up::Random { state: seed };
    let mut file = path.map(|path| std::fs::File::create(path).expect("the file for the blocks"));

    let encoder = Bc7e::new();
    let pixels = make_blocks(&mut random, blocks);
    let mut packed = vec![[0u8; 16]; blocks as usize];
    let mut modes = [0u32; 9];

    if presets {
        for (name, profile) in PROFILES {
            for perceptual in [false, true] {
                encoder.compress_blocks(&mut packed, &pixels, &profile(perceptual));
                println!("{name} {} {:016x}", if perceptual { "perceptual" } else { "linear" }, hash(&packed, &mut modes));
                if let Some(file) = &mut file {
                    file.write_all(packed.as_flattened()).expect("writing the blocks");
                }
            }
        }
    } else {
        let mut first = 0usize;
        while first < blocks as usize {
            let count = (blocks as usize - first).min(32);
            let params = make_settings(&mut random);
            encoder.compress_blocks(&mut packed[first..first + count], &pixels[first..first + count], &params);
            first += count;
        }
        println!("settings {:016x}", hash(&packed, &mut modes));
        if let Some(file) = &mut file {
            file.write_all(packed.as_flattened()).expect("writing the blocks");
        }
    }
    print!("blocks by mode:");
    for count in modes {
        print!(" {count}");
    }
    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(what), Some(seed), Some(blocks)) = (args.get(1), args.get(2).and_then(|s| s.parse::<u64>().ok()), args.get(3).and_then(|s| s.parse::<u32>().ok())) else {
        eprintln!("usage: dump <what> <seed> <blocks>   (see the top of dump.rs)");
        std::process::exit(2);
    };
    match what.as_str() {
        "bc7decomp" => dump_bc7decomp(seed, blocks),
        "unpack" => dump_unpack(seed, blocks),
        "bc1" => dump_bc1(seed, blocks),
        "bc345" => dump_bc345(seed, blocks),
        "bc7enc" => dump_bc7enc(seed, blocks),
        "bc7block" => dump_bc7block(seed, blocks),
        "ert" => print!("{}", made_up::ert(seed, blocks)),
        "rdo" => print!("{}", made_up::rdo(seed, blocks, false)),
        "rdo_bc7e" => print!("{}", made_up::rdo(seed, blocks, true)),
        "bc2" => print!("{}", made_up::bc2(seed, blocks)),
        "bc6h_enc" => print!("{}", made_up::bc6h_enc(seed, blocks)),
        "miniz" => print!("{}", made_up::miniz(seed, blocks)),
        "stb_image_write" => print!("{}", made_up::stb_image_write(seed, blocks)),
        "stb_image" => print!("{}", made_up::stb_image(seed, blocks)),
        "stb_image_more" => print!("{}", made_up::stb_image_more(seed, blocks)),
        "zstd_decode" | "zstd_decode_small" => {
            let Some(file) = args.get(4).and_then(|path| std::fs::read(path).ok()) else {
                eprintln!("usage: dump {what} <seed> <count> <file of frames from compare/dump_zstd.c>");
                std::process::exit(2);
            };
            print!("{}", made_up::zstd_decode(what, blocks, &file));
        }
        "tinyexr_write" | "tinyexr_write_small" => print!("{}", made_up::tinyexr_write(what, seed, blocks)),
        "tinyexr_read" | "tinyexr_read_small" => {
            let Some(file) = args.get(4).and_then(|path| std::fs::read(path).ok()) else {
                eprintln!("usage: dump {what} <seed> <count> <file of files from compare/dump_tinyexr.cc>");
                std::process::exit(2);
            };
            print!("{}", made_up::tinyexr_read(what, blocks, &file));
        }
        "tinyexr_parts" | "tinyexr_deep" => {
            let Some(file) = args.get(4).and_then(|path| std::fs::read(path).ok()) else {
                eprintln!("usage: dump {what} <seed> <count> <file of files from compare/dump_tinyexr.cc>");
                std::process::exit(2);
            };
            if what == "tinyexr_parts" {
                print!("{}", made_up::tinyexr_parts(what, seed, blocks, &file));
            } else {
                print!("{}", made_up::tinyexr_deep(what, blocks, &file));
            }
        }
        "zstd_compress" | "zstd_compress_small" => print!("{}", made_up::zstd_compress(what, seed, blocks, false)),
        "zstd_compress_long" => print!("{}", made_up::zstd_compress_long(what, seed, blocks, false)),
        "zstd_compress_more" | "zstd_compress_more_small" => {
            let highest = args.get(4).map_or(9, |arg| arg.parse().expect("a strategy"));
            print!("{}", made_up::zstd_compress_up_to(what, seed, blocks, false, highest));
        }
        "zstd_entropy" => print!("{}", made_up::zstd_entropy(seed, blocks)),
        "bc6h_unpack" => print!("{}", made_up::bc6h_unpack(seed, blocks)),
        "bc6h_pack" => print!("{}", made_up::bc6h_pack(seed, blocks)),
        "bc7e_presets" => dump_bc7e(true, seed, blocks, args.get(4)),
        "bc7e_settings" => dump_bc7e(false, seed, blocks, args.get(4)),
        _ => std::process::exit(2),
    }
}
