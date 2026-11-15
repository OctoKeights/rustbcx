//! The port of bc7e against results recorded from bc7e.ispc itself, so that a change shows
//! without the ISPC compiler at hand.
//!
//! `tests/data/bc7e_expected.txt` is what `compare/dump_bc7e.cpp`, built against the strict
//! build of bc7e.ispc (see `compare/compare.sh`), prints for `bc7e_presets 1 300` and then
//! `bc7e_settings 1 3000`: a hash of the packed blocks for each of the seven profiles with each
//! error measure, and one for 3,000 blocks packed with 94 made-up settings.
//! `compare/compare.sh` is the full comparison.

mod bc7e_made_up;

use bc7e_made_up::{hash, make_blocks, make_settings, Random, PROFILES};
use rustbcx::bc7e::Bc7e;

#[test]
fn packs_as_the_ispc_encoder_does() {
    let encoder = Bc7e::new();
    let mut printed = String::new();

    // bc7e_presets 1 300
    let mut random = Random { state: 1 };
    let pixels = make_blocks(&mut random, 300);
    let mut packed = vec![[0u8; 16]; pixels.len()];
    let mut modes = [0u32; 9];
    for (name, profile) in PROFILES {
        for perceptual in [false, true] {
            encoder.compress_blocks(&mut packed, &pixels, &profile(perceptual));
            printed += &format!("{name} {} {:016x}\n", if perceptual { "perceptual" } else { "linear" }, hash(&packed, &mut modes));
        }
    }
    printed += &format!("blocks by mode:{}\n", modes.map(|count| format!(" {count}")).concat());

    // bc7e_settings 1 3000
    let mut random = Random { state: 1 };
    let pixels = make_blocks(&mut random, 3000);
    let mut packed = vec![[0u8; 16]; pixels.len()];
    let mut modes = [0u32; 9];
    for (packed, pixels) in packed.chunks_mut(32).zip(pixels.chunks(32)) {
        encoder.compress_blocks(packed, pixels, &make_settings(&mut random));
    }
    printed += &format!("settings {:016x}\n", hash(&packed, &mut modes));
    printed += &format!("blocks by mode:{}\n", modes.map(|count| format!(" {count}")).concat());

    let expected = include_str!("data/bc7e_expected.txt");
    for (line, (ours, theirs)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(ours, theirs, "line {}", line + 1);
    }
    assert_eq!(printed.lines().count(), expected.lines().count());
    // Every mode is in there.
    assert!(modes[..8].iter().all(|&count| count > 0), "{modes:?}");
}
