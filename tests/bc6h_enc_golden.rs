//! The port of `bc6h_enc` against results recorded from the original, so that a change shows
//! without a C++ compiler at hand.
//!
//! `tests/data/bc6h_enc_expected.txt` is what `compare/dump_bc6h_enc.cpp`, built against
//! `bc6h_enc.h` (see `compare/compare.sh`), prints for `bc6h_enc 9 240`: a hash of the blocks
//! made-up HDR pixels of both signs pack to, as signed and as unsigned BC6H, and how many
//! blocks came out in each mode. `compare/compare.sh` is the full comparison.

mod made_up;

#[test]
fn packs_as_bc6h_enc_does() {
    let printed = made_up::bc6h_enc(9, 240);
    let expected = include_str!("data/bc6h_enc_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
