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

`ert_expected.txt` is what bc7enc_rdo's C++ makes of made-up blocks put through its entropy
reduction transform, as `compare/dump.cpp` prints it; `tests/ert_golden.rs` checks that the port
prints the same. To make it again:

    compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "ert"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-cpp"
    for run in "3 1200" "7 600" "12 900"; do "$ref" ert $run; done > tests/data/ert_expected.txt

`rdo_expected.txt` is what bc7enc_rdo's image encoder makes of made-up images, with bc7e.ispc
built the strict way for part of the BC7; `tests/rdo_golden.rs` checks that the port prints the
same. To make it again:

    compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "rdo rdo_bc7e"
    tmp="${TMPDIR:-/tmp}/rustbcx-compare"
    { "$tmp/dump-cpp" rdo 9 800; "$tmp/dump-cpp-bc7e" rdo_bc7e 2 800; "$tmp/dump-cpp-bc7e" rdo_bc7e 14 800; } > tests/data/rdo_expected.txt

`bc6h_expected.txt` is what bcdec makes of made-up BC6H blocks, and what the ISPC Texture
Compressor's BC6H packer, built the strict way, makes of made-up HDR pixels;
`tests/bc6h_golden.rs` checks that the port prints the same. To make it again:

    BCDEC=<folder holding bcdec.h> ISPCTC=<checkout of ISPCTextureCompressor> \
        compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "bc6h_unpack bc6h_pack"
    tmp="${TMPDIR:-/tmp}/rustbcx-compare"
    { "$tmp/dump-bc6h" bc6h_unpack 5 20000; "$tmp/dump-bc6h" bc6h_unpack 11 20000; "$tmp/dump-bc6h-ispc" bc6h_pack 4 1500; } > tests/data/bc6h_expected.txt

`bc2_expected.txt` is what bc7enc_rdo's rgbcx and bcdec together make of made-up BC2 blocks and
pixels, as `compare/dump.cpp` built with bcdec prints it; `tests/bc2_golden.rs` checks that the
crate prints the same. To make it again:

    BCDEC=<folder holding bcdec.h> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "bc2"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-cpp-bc2"
    { "$ref" bc2 6 20000; "$ref" bc2 13 20000; } > tests/data/bc2_expected.txt

`bc6h_enc_expected.txt` is what bc6h_enc.h makes of made-up HDR pixels, as
`compare/dump_bc6h_enc.cpp` prints it; `tests/bc6h_enc_golden.rs` checks that the port prints
the same. To make it again:

    BC6HENC=<folder holding bc6h_enc.h> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "bc6h_enc"
    "${TMPDIR:-/tmp}/rustbcx-compare/dump-bc6h-enc" bc6h_enc 9 240 > tests/data/bc6h_enc_expected.txt

`miniz_expected.txt` is what miniz makes of made-up data, as `compare/dump_miniz.c` prints it;
`tests/miniz_golden.rs` checks that the port prints the same. To make it again:

    MINIZ=<checkout of miniz> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "miniz"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-miniz"
    { "$ref" miniz 3 120; "$ref" miniz 8 120; } > tests/data/miniz_expected.txt

`stb_image_write_expected.txt` is what stb_image_write.h makes of made-up images, as
`compare/dump_stb_image_write.c` prints it; `tests/stb_image_write_golden.rs` checks that the
port prints the same. To make it again:

    STB=<checkout of stb> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "stb_image_write"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-stb-image-write"
    { "$ref" stb_image_write 5 400; "$ref" stb_image_write 12 400; } > tests/data/stb_image_write_expected.txt

`stb_image_expected.txt` is what stb_image.h makes of made-up `.png`, `.bmp` and `.jpg` files,
as `compare/dump_stb_image.c` prints it; `tests/stb_image_golden.rs` checks that the port
prints the same. To make it again:

    STB=<checkout of stb> MINIZ=<checkout of miniz> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "stb_image"
    ref="${TMPDIR:-/tmp}/rustbcx-compare/dump-stb-image"
    { "$ref" stb_image 4 600; "$ref" stb_image 10 600; } > tests/data/stb_image_expected.txt

`zstd_frames.bin` holds Zstandard frames that the zstd library made of made-up data, as
`compare/dump_zstd.c` wrote them, and `zstd_decode_expected.txt` what zstd's reference decoder
makes of each, as the same program prints it; `tests/zstd_golden.rs` checks that the port
prints the same from the same frames. To make them again:

    ZSTD=<checkout of zstd> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "zstd_decode"
    "${TMPDIR:-/tmp}/rustbcx-compare/dump-zstd" zstd_decode_small 6 120 tests/data/zstd_frames.bin > tests/data/zstd_decode_expected.txt

`zstd_compress_expected.txt` is what `compare/dump_zstd_compress.c` prints of the frames the
zstd library's `ZSTD_compress` makes of made-up data, and `zstd_entropy_expected.txt` what
`compare/dump_zstd_entropy.c` prints of the answers of the entropy coders inside that
library, asked one function at a time; `tests/zstd_compress_golden.rs` checks that the port
of the compressor prints the same. To make them again:

    ZSTD=<checkout of zstd> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "zstd_compress zstd_entropy"
    "${TMPDIR:-/tmp}/rustbcx-compare/dump-zstd-compress" zstd_compress_small 7 200 > tests/data/zstd_compress_expected.txt
    "${TMPDIR:-/tmp}/rustbcx-compare/dump-zstd-entropy" zstd_entropy 9 4000 > tests/data/zstd_entropy_expected.txt

`exr_files.bin` holds `.exr` files that tinyexr saved of made-up images, as
`compare/dump_tinyexr.cc` wrote them, and `tinyexr_read_expected.txt` what tinyexr reads of
each, as the same program prints it; `tests/tinyexr_golden.rs` checks that the port prints the
same from the same files. To make them again:

    TINYEXR=<checkout of tinyexr> MINIZ=<checkout of miniz> compare/compare.sh <checkout of bc7enc_rdo> 3000 1 "tinyexr_read"
    "${TMPDIR:-/tmp}/rustbcx-compare/dump-tinyexr" tinyexr_read_small 11 100 tests/data/exr_files.bin > tests/data/tinyexr_read_expected.txt

No texture or picture from anywhere is in this repository: every block the tests and the
comparison pack, and every file they read, is made up from a seed.
