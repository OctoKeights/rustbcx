//! The port of the entropy reduction transform against results recorded from the C++, so that
//! a change shows without a C++ compiler at hand.
//!
//! `tests/data/ert_expected.txt` is what `compare/dump.cpp`, built against `bc7enc_rdo` (see
//! `compare/compare.sh`), prints for `ert 3 1200`, `ert 7 600` and `ert 12 900`: a hash of the
//! blocks after the transform and how many it changed. `compare/compare.sh` is the full
//! comparison.

mod made_up;

#[test]
fn reduces_entropy_as_the_cpp_does() {
    let printed = [(3, 1200), (7, 600), (12, 900)]
        .map(|(seed, blocks)| made_up::ert(seed, blocks))
        .concat();
    let expected = include_str!("data/ert_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
