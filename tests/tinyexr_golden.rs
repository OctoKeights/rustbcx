//! The port of tinyexr's reader against results recorded from the original, so that a change
//! shows without a C++ compiler at hand.
//!
//! `tests/data/exr_files.bin` holds the files `compare/dump_tinyexr.cc` made with tinyexr for
//! `tinyexr_read_small 11 100` (see `compare/compare.sh`): made-up images saved as scanlines
//! and as tiles, with every compression tinyexr writes, a third of them damaged.
//! `tests/data/tinyexr_read_expected.txt` is what that program printed for them: a hash of
//! what tinyexr reads of each, or of the message it refuses it with.

mod made_up;

#[test]
fn reads_as_tinyexr_does() {
    let printed = made_up::tinyexr_read("tinyexr_read_small", 100, include_bytes!("data/exr_files.bin"));
    let expected = include_str!("data/tinyexr_read_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}

/// Files damaged far worse than the comparison's: whatever is in them, the reader gives an
/// image or a message, and does not panic. (`ROUNDS` over the recorded files; more by hand
/// when the reader is changed.)
#[test]
fn badly_damaged_files_are_read_or_refused() {
    use rustbcx::tinyexr;
    const ROUNDS: u32 = 12;
    let file = include_bytes!("data/exr_files.bin");
    let mut r = made_up::Random { state: 3 };
    let (mut read, mut refused) = (0, 0);
    for _ in 0..ROUNDS {
        let mut at = 0;
        while at < file.len() {
            let number = |at: &mut usize| {
                *at += 4;
                u32::from_le_bytes(file[*at - 4..*at].try_into().unwrap()) as usize
            };
            let _wish = number(&mut at);
            let size = number(&mut at);
            let mut bytes = file[at..at + size].to_vec();
            at += size;
            if bytes.is_empty() {
                continue;
            }
            // A few bytes made up, more often near the start, where the header and the table
            // of offsets are; now and then the end cut off.
            for _ in 0..1 + r.next() % 4 {
                let within = if r.next().is_multiple_of(2) { bytes.len().min(500) } else { bytes.len() };
                let place = r.next() as usize % within;
                bytes[place] = if r.next().is_multiple_of(4) { 0 } else { r.next() as u8 };
            }
            if r.next().is_multiple_of(8) {
                bytes.truncate(r.next() as usize % bytes.len());
            }

            let Ok(version) = tinyexr::parse_exr_version_from_memory(&bytes) else {
                refused += 1;
                continue;
            };
            let Ok(mut header) = tinyexr::parse_exr_header_from_memory(&version, &bytes) else {
                refused += 1;
                continue;
            };
            // (Not the sizes that ask for more memory than a test should take.)
            if made_up::tinyexr_left(&header) {
                continue;
            }
            match tinyexr::load_exr_image_from_memory(&mut header, &bytes) {
                Ok(_) => read += 1,
                Err(_) => refused += 1,
            }
            let _ = tinyexr::load_exr_from_memory(&bytes);
        }
    }
    assert!(read > 0 && refused > 0);
}
