# Recorded results

`bc7e_expected.txt` is what the original `bc7e.ispc` makes of made-up blocks, as
`compare/dump_bc7e.cpp` prints it: a hash of the packed blocks for each of the seven profiles
with each error measure, then one for 3,000 blocks packed with made-up settings.
`tests/bc7e_golden.rs` checks that the port prints the same, so `cargo test` shows a change
without the ISPC compiler at hand.

To make it again, run the comparison once (it builds the reference), then the reference by
itself:

    compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "bc7e_presets"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-ispc"
    "$ref" bc7e_presets 1 300 > tests/data/bc7e_expected.txt
    "$ref" bc7e_settings 1 3000 >> tests/data/bc7e_expected.txt

No texture from anywhere is in this repository: every block the tests and the comparison pack
is made up from a seed.
