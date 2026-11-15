# Changelog

## 0.1.0

- First release: the port of bc7enc_rdo's `rgbcx` (BC1, BC3, BC4 and BC5, both ways),
  `bc7decomp` (BC7 to pixels) and `bc7enc` (pixels to BC7 in modes 1, 5, 6 and 7), and of
  `bc7e.ispc` (pixels to BC7 in all eight modes, with its seven profiles).
- Every block is the one the originals produce; for `bc7e`, the original built without
  fast-math. `compare/compare.sh` checks it against both.
