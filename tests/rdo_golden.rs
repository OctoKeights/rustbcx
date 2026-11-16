//! The port of the image encoder against results recorded from the C++, so that a change shows
//! without a C++ compiler or ISPC at hand.
//!
//! `tests/data/rdo_expected.txt` is what `compare/dump.cpp`, built against bc7enc_rdo and the
//! strict build of bc7e.ispc (see `compare/compare.sh`), prints for `rdo 9 800`,
//! `rdo_bc7e 2 800` and `rdo_bc7e 14 800`: a hash of the blocks of made-up images before and
//! after the rate-distortion pass and of what they unpack to. `compare/compare.sh` is the full
//! comparison.

mod made_up;

#[test]
fn packs_images_as_the_cpp_does() {
    let printed = [
        made_up::rdo(9, 800, false),
        made_up::rdo(2, 800, true),
        made_up::rdo(14, 800, true),
    ]
    .concat();
    let expected = include_str!("data/rdo_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
}
