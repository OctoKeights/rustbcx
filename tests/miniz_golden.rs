//! The port of miniz against results recorded from the original, so that a change shows
//! without a C compiler at hand.
//!
//! `tests/data/miniz_expected.txt` is what `compare/dump_miniz.c`, built against miniz (see
//! `compare/compare.sh`), prints for `miniz 3 120` and `miniz 8 120`: a hash of what made-up
//! data compresses to with every kind of flags, in one go and in pieces, and of what each
//! stream, whole and damaged, decompresses to or is refused with. `compare/compare.sh` is the
//! full comparison.

mod made_up;

#[test]
fn compresses_and_decompresses_as_miniz_does() {
    let printed = [made_up::miniz(3, 120), made_up::miniz(8, 120)].concat();
    let expected = include_str!("data/miniz_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
