//! zlib and deflate, both ways: the port of Rich Geldreich's
//! [miniz](https://github.com/richgel999/miniz), the parts that compress and decompress.
//!
//! - [`tdef`]: the compressor (`miniz_tdef.c`).
//! - [`tinfl`]: the decompressor (`miniz_tinfl.c`).
//! - Here: the two checksums (`miniz.c`), and [`compress`] and [`uncompress`], the short way
//!   to and from a zlib stream.
//!
//! The compressor gives the bytes miniz gives, at every level and with every flag, and the
//! decompressor takes and refuses the streams miniz takes and refuses. Left out are miniz's
//! other parts: its copy of zlib's stream functions (`mz_deflate`, `mz_inflate` and theirs),
//! `.zip` archives, and its PNG writer.
//!
//! This crate uses it for `.ktx2` files with ZLIB supercompression.

pub mod tdef;
pub mod tinfl;

mod tables;

/// What [`mz_adler32`] starts from: `MZ_ADLER32_INIT`.
pub const MZ_ADLER32_INIT: u32 = 1;
/// What [`mz_crc32`] starts from: `MZ_CRC32_INIT`.
pub const MZ_CRC32_INIT: u32 = 0;

/// The Adler-32 checksum of some bytes, carried on from `adler` ([`MZ_ADLER32_INIT`] to
/// begin): `mz_adler32`.
pub fn mz_adler32(adler: u32, ptr: &[u8]) -> u32 {
    let (mut s1, mut s2) = (adler & 0xffff, adler >> 16);
    // (5,552 is the most bytes the two sums can take before they have to be brought down.)
    let first = ptr.len() % 5552;
    let (head, rest) = ptr.split_at(first);
    for block in std::iter::once(head).chain(rest.chunks(5552)) {
        for &byte in block {
            s1 += u32::from(byte);
            s2 += s1;
        }
        s1 %= 65521;
        s2 %= 65521;
    }
    (s2 << 16) + s1
}

/// The CRC-32 checksum of some bytes, carried on from `crc` ([`MZ_CRC32_INIT`] to begin):
/// `mz_crc32`.
pub fn mz_crc32(crc: u32, ptr: &[u8]) -> u32 {
    let mut crc32 = crc ^ 0xFFFF_FFFF;
    for &byte in ptr {
        crc32 = (crc32 >> 8) ^ tables::S_CRC_TABLE[((crc32 ^ u32::from(byte)) & 0xFF) as usize];
    }
    !crc32
}

/// Compresses some bytes to a zlib stream: what `mz_compress2` writes. `level` goes from 0 (no
/// compression) through 1 (fastest) and 6 (the usual) to 9 (best), with 10 for more than that
/// at any cost in time. Below 0 is miniz's default, which is not quite level 6: it searches
/// as level 6 does and parses as the low levels do.
pub fn compress(source: &[u8], level: i32) -> Vec<u8> {
    tdef::tdefl_compress_mem_to_heap(
        source,
        tdef::TDEFL_COMPUTE_ADLER32
            | tdef::tdefl_create_comp_flags_from_zip_params(level, 15, tdef::MZ_DEFAULT_STRATEGY),
    )
}

/// Decompresses a zlib stream that holds no more than `max_len` bytes. None when the stream is
/// damaged or cut short, when its checksum is wrong, and when it holds more than that.
pub fn uncompress(source: &[u8], max_len: usize) -> Option<Vec<u8>> {
    tinfl::tinfl_decompress_mem_to_mem(source, max_len, tinfl::TINFL_FLAG_PARSE_ZLIB_HEADER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksums_are_the_known_ones() {
        assert_eq!(mz_adler32(MZ_ADLER32_INIT, b""), 1);
        assert_eq!(mz_adler32(MZ_ADLER32_INIT, b"Wikipedia"), 0x11E6_0398);
        assert_eq!(mz_crc32(MZ_CRC32_INIT, b""), 0);
        assert_eq!(mz_crc32(MZ_CRC32_INIT, b"123456789"), 0xCBF4_3926);
        // In pieces as in one go, across the place the Adler sums are brought down.
        let data: Vec<u8> = (0..20_000u32)
            .map(|i| (i * 7 + i / 255) as u8 | 0x80)
            .collect();
        let whole = (mz_adler32(1, &data), mz_crc32(0, &data));
        let (a, b) = data.split_at(5553);
        assert_eq!(
            (mz_adler32(mz_adler32(1, a), b), mz_crc32(mz_crc32(0, a), b)),
            whole
        );
    }

    #[test]
    fn bytes_go_in_and_come_out_at_every_level() {
        let mut data = Vec::new();
        let mut state = 7u32;
        for i in 0..70_000u32 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            // Runs, repeats and noise.
            data.push(match (i / 3000) % 4 {
                0 => (i / 50) as u8,
                1 => b"the quick brown fox "[(i % 20) as usize],
                2 => (state >> 24) as u8,
                _ => (state >> 30) as u8,
            });
        }
        let mut sizes = Vec::new();
        for level in -1..=10 {
            let packed = compress(&data, level);
            assert_eq!(
                uncompress(&packed, data.len()).as_deref(),
                Some(&data[..]),
                "level {level}"
            );
            sizes.push(packed.len());
        }
        // Level 0 stores, and the best level is the smallest.
        assert!(sizes[1] > data.len() && sizes[10] < data.len() / 2 && sizes[10] <= sizes[2]);
        // Nothing in, a stream of nothing out.
        let empty = compress(&[], 6);
        assert_eq!(uncompress(&empty, 0), Some(Vec::new()));
        // Too small a limit, a damaged byte, a missing end: all refused.
        let packed = compress(&data, 6);
        assert!(uncompress(&packed, data.len() - 1).is_none());
        assert!(uncompress(&packed[..packed.len() - 1], data.len()).is_none());
        let mut damaged = packed.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert!(uncompress(&damaged, data.len()).is_none());
    }

    #[test]
    fn damaged_streams_are_refused_or_read_and_never_more_than_asked() {
        // Made-up bytes, and good streams with bytes changed: whatever comes of them, nothing
        // overflows and no more than the room given comes out.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let good = tdef::tdefl_compress_mem_to_heap(
            &(0..3000u32)
                .map(|i| (i % 251) as u8 ^ (i / 97) as u8)
                .collect::<Vec<u8>>(),
            128,
        );
        for i in 0..6000 {
            let stream: Vec<u8> = if i % 2 == 0 {
                (0..1 + next() % 80).map(|_| next() as u8).collect()
            } else {
                let mut stream = good.clone();
                for _ in 0..1 + next() % 4 {
                    let at = next() as usize % stream.len();
                    stream[at] = next() as u8;
                }
                stream
            };
            let room = next() as usize % 5000;
            if let Ok((out, taken)) = tinfl::tinfl_decompress(&stream, room, 0) {
                assert!(out.len() <= room && taken <= stream.len());
            }
            let _ = uncompress(&stream, room);
        }
    }

    #[test]
    fn input_in_pieces_gives_the_same_stream() {
        let data: Vec<u8> = (0..100_000u32)
            .map(|i| (i.wrapping_mul(i) >> 7) as u8 ^ (i / 1000) as u8)
            .collect();
        let flags = tdef::tdefl_create_comp_flags_from_zip_params(6, 15, tdef::MZ_DEFAULT_STRATEGY);
        let whole = tdef::tdefl_compress_mem_to_heap(&data, flags);
        let mut comp = tdef::TdeflCompressor::new(flags);
        for piece in data.chunks(777) {
            assert_eq!(
                comp.compress_buffer(piece, tdef::TdeflFlush::NoFlush),
                tdef::TdeflStatus::Okay
            );
        }
        assert_eq!(
            comp.compress_buffer(&[], tdef::TdeflFlush::Finish),
            tdef::TdeflStatus::Done
        );
        assert_eq!(comp.take_output(), whole);
        // Once finished, it takes no more.
        assert_eq!(
            comp.compress_buffer(b"more", tdef::TdeflFlush::Finish),
            tdef::TdeflStatus::BadParam
        );
        assert_eq!(comp.prev_return_status(), tdef::TdeflStatus::BadParam);
        assert!(comp.take_output().is_empty());
        // A sync flush in the middle leaves a stream that still holds everything.
        let mut comp = tdef::TdeflCompressor::new(flags);
        comp.compress_buffer(&data[..40_000], tdef::TdeflFlush::SyncFlush);
        comp.compress_buffer(&data[40_000..], tdef::TdeflFlush::Finish);
        assert_eq!(
            uncompress(&comp.take_output(), data.len()).as_deref(),
            Some(&data[..])
        );
    }
}
