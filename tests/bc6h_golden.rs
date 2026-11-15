//! The port of the BC6H code against results recorded from the originals, so that a change
//! shows without a C++ compiler at hand.
//!
//! `tests/data/bc6h_expected.txt` is what `compare/dump_bc6h.cpp`, built against bcdec (see
//! `compare/compare.sh`), prints for `bc6h_unpack 5 20000` and `bc6h_unpack 11 20000`: a hash
//! of what made-up blocks unpack to. `compare/compare.sh` is the full comparison.

mod made_up;

#[test]
fn bc6h_as_the_originals_do_it() {
    let printed = [made_up::bc6h_unpack(5, 20000), made_up::bc6h_unpack(11, 20000)].concat();
    let expected = include_str!("data/bc6h_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
