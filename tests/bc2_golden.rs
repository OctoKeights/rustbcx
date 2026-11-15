//! BC2 against results recorded from the originals it is made of, so that a change shows
//! without a C++ compiler at hand.
//!
//! `tests/data/bc2_expected.txt` is what `compare/dump.cpp`, built against bc7enc_rdo and
//! bcdec together (see `compare/compare.sh`), prints for `bc2 6 20000` and `bc2 13 20000`: a
//! hash of what made-up BC2 blocks unpack to (the alphas by bcdec, the colours by rgbcx) and
//! of the blocks made-up pixels pack to. `compare/compare.sh` is the full comparison.

mod made_up;

#[test]
fn bc2_as_the_originals_do_it() {
    let printed = [made_up::bc2(6, 20000), made_up::bc2(13, 20000)].concat();
    let expected = include_str!("data/bc2_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
