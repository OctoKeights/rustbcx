//! The ports of libdeflate and of OpenEXRCore's DWAA and DWAB against results recorded from
//! the originals, so that a change shows without a C compiler at hand.
//!
//! - `tests/data/libdeflate_expected.txt` is what `compare/dump_libdeflate.c`, built against
//!   libdeflate, prints for `libdeflate 3 200` and `libdeflate 8 200`: a hash of what made-up
//!   data compresses to at each level, and of what each stream, whole and damaged,
//!   decompresses to or is refused with.
//! - `tests/data/dwa_write_expected.txt` is what `compare/dump_dwa.c`, built against
//!   OpenEXRCore, prints for `dwa_write 3 400` and `dwa_write 8 400`: a hash of made-up
//!   blocks of an image as OpenEXRCore compresses them with DWAA and DWAB, and of what it
//!   decompresses them to; `tests/data/dwa_read_expected.txt` for `dwa_read 3 600` and
//!   `dwa_read 8 600`, of what it makes of them damaged.
//!
//! `compare/compare.sh` is the full comparison.

mod made_up;

fn same(printed: String, expected: &str) {
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}

#[test]
fn compresses_and_decompresses_as_libdeflate_does() {
    same([made_up::libdeflate(3, 200), made_up::libdeflate(8, 200)].concat(), include_str!("data/libdeflate_expected.txt"));
}

#[test]
fn compresses_blocks_as_openexr_does() {
    same([made_up::dwa("dwa_write", 3, 400), made_up::dwa("dwa_write", 8, 400)].concat(), include_str!("data/dwa_write_expected.txt"));
}

#[test]
fn decompresses_damaged_blocks_as_openexr_does() {
    same([made_up::dwa("dwa_read", 3, 600), made_up::dwa("dwa_read", 8, 600)].concat(), include_str!("data/dwa_read_expected.txt"));
}
