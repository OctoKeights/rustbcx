#!/bin/sh
# Compares the Rust port with the originals: bc7enc_rdo's C++ library and bc7e.ispc.
#
#   compare/compare.sh <folder with bc7enc_rdo's sources> [blocks] [seeds] [dumps]
#
# The folder is a checkout of https://github.com/richgel999/bc7enc_rdo: it holds both
# originals. Runs every dump of both sides for each seed and compares what they print byte for
# byte. [dumps] names the dumps to run, in quotes, when not all of them are wanted.
#
# The C++ library (bc7decomp.cpp, bc7enc.cpp, rgbcx.cpp, ert.cpp, rdo_bc_encoder.cpp and what
# they need): compare/dump.cpp is built against it. Dumps: bc7decomp unpack bc1 bc345 bc7enc
# bc7block ert rdo (ert and rdo with a tenth of [blocks]: they unpack each block thousands of
# times).
#
# bc7e.ispc: needs the ISPC compiler (https://github.com/ispc/ispc/releases), `ispc` on the
# path or its path in $ISPC. bc7e.ispc is built the strict way (for SSE2, without fast-math:
# no fused multiply-add, no reordered arithmetic), which gives the same blocks on every
# machine, and compare/dump_bc7e.cpp is built against it. Dumps: bc7e_presets bc7e_settings,
# each with a tenth of [blocks], since it packs every block fourteen times or with thorough
# settings; and rdo_bc7e, the rdo dump with compare/dump.cpp built against the C++ library and
# that bc7e together. (A build of bc7e.ispc with --opt=fast-math, or for AVX2 without
# --opt=disable-fma, packs some blocks differently from this one, and so from the port.)
#
# BC6H: the unpacker is compared with bcdec (https://github.com/iOrange/bcdec), whose folder
# (the one holding bcdec.h) goes in $BCDEC; compare/dump_bc6h.cpp is built against it. Dump:
# bc6h_unpack. The packer is compared with Intel's ISPC Texture Compressor
# (https://github.com/GameTechDev/ISPCTextureCompressor), whose folder goes in $ISPCTC; its
# kernel.ispc is built the strict way, as bc7e.ispc is, and compare/dump_bc6h.cpp against that
# and bcdec. Dump: bc6h_pack, with a tenth of [blocks], since it packs every block five times.
#
# BC2, which bc7enc_rdo does not have: the crate's own, made of rgbcx's BC1 for the colours and
# sixteen alphas of four bits. Compared with compare/dump.cpp built against the C++ library
# and bcdec together: the alphas with bcdec's bcdec_bc2, the colours with rgbcx's BC1 (bcdec
# rounds its colours another way than any of rgbcx's four, so the dump only checks that they
# are near). Dump: bc2.
#
# The other BC6H packer, of signed blocks as well as unsigned, is compared with bc6h_enc
# (https://github.com/0xc0de/bc6h_enc), DirectXTex's encoder as one file, whose folder goes in
# $BC6HENC; compare/dump_bc6h_enc.cpp is built against it with BC6H_SSE_INTRINSICS, the build
# that does what DirectXTex does (it needs an x86-64 processor with SSE4.1 and F16C, which all
# since 2013 have). Dump: bc6h_enc, with a fortieth of [blocks], since it is a slow encoder and
# packs every block twice.
#
# zlib and deflate (src/miniz) are compared with miniz (https://github.com/richgel999/miniz),
# whose checkout goes in $MINIZ; compare/dump_miniz.c is built against its compressor and its
# decompressor as they build by default. Dump: miniz, with a hundredth of [blocks] as the
# number of made-up files, each some tens of thousands of bytes, compressed three ways and
# decompressed seven.
#
# The image files (src/stb_image_write.rs, src/stb_image) are compared with stb
# (https://github.com/nothings/stb), whose checkout goes in $STB. compare/dump_stb_image_write.c
# is built against its stb_image_write.h, without contraction of a multiply and an add (its
# JPEG writer works in floats). Dump: stb_image_write, with a tenth of [blocks] as the number
# of made-up images, each written as .png, .bmp and .jpg. compare/dump_stb_image.c is built
# against its stb_image.h as well, for the three formats only and with STBI_NO_SIMD (the
# plain C, which the port is of), with memory that starts as zeros and with overflow that
# wraps (-fwrapv), so that the damaged files among those it reads have one answer; it needs
# $MINIZ too, to compress the files it makes up. Dump: stb_image, with a tenth of [blocks] as
# the number of made-up files, each asked its size and read once or twice.
#
# Zstandard (src/zstd) is compared with zstd (https://github.com/facebook/zstd), whose checkout
# goes in $ZSTD. Its library is built from that (without assembly and without threads), and
# compare/dump_zstd.c against it, to make frames of every kind; the decoder is compared with
# the reference decoder in its doc/educational_decoder, which compare/dump_zstd_edu.c wraps
# so that a damaged frame gives its message back in place of stopping the program. Dump:
# zstd_decode, with a tenth of [blocks] as the number of frames. The C side writes its frames
# to a file, which the Rust side reads.
# The compressor is compared with that library's own: compare/dump_zstd_compress.c has
# ZSTD_compress make frames of made-up data at the levels that are ported, and
# compare/dump_zstd_entropy.c asks the entropy coders inside it one function at a time, for
# what a whole frame seldom or never asks of them. Dumps: zstd_compress, with a hundredth of
# [blocks] as the number of sources (of up to five megabytes each), and zstd_entropy, with
# [blocks] as the number of rounds.
#
# Without the ISPC compiler the bc7e dumps and bc6h_pack are left out, without $BCDEC bc2 and
# the BC6H dumps, without $ISPCTC bc6h_pack, without $BC6HENC bc6h_enc, without $MINIZ miniz
# and stb_image, without $STB stb_image_write and stb_image, and without $ZSTD the three zstd
# dumps; the script says so and fails.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <folder with bc7enc_rdo's sources> [blocks] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-bc7decomp unpack bc1 bc345 bc7enc bc7block ert rdo bc7e_presets bc7e_settings rdo_bc7e bc2 bc6h_unpack bc6h_pack bc6h_enc miniz stb_image_write stb_image zstd_decode zstd_compress zstd_entropy}
tmp=${TMPDIR:-/tmp}/rustbcx-compare
mkdir -p "$tmp" || exit 2

(cd "$here" && cargo build --release -q --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

built_cpp=0
built_ispc=0
built_bc2=0
built_bc6h=0
built_bc6h_enc=0
built_miniz=0
built_stbiw=0
built_stbi=0
built_zstd=0
built_bc6h_pack=0
bad=0
# (ert.h uses the fixed-width integer types without including their header; no contraction of
# a multiply and an add into one instruction, which Rust never does.)
cpp_flags="-O2 -ffp-contract=off -include stdint.h"
cpp_sources="$source/bc7decomp.cpp $source/bc7decomp_ref.cpp $source/bc7enc.cpp $source/rgbcx.cpp $source/ert.cpp $source/rdo_bc_encoder.cpp $source/utils.cpp $source/lodepng.cpp"
for what in $dumps; do
    extra=
    case $what in
    zstd_decode | zstd_compress | zstd_entropy)
        if [ "$built_zstd" -eq 0 ]; then
            if [ -z "${ZSTD:-}" ] || [ ! -f "$ZSTD/doc/educational_decoder/zstd_decompress.c" ]; then
                echo "$what: NOT COMPARED, no zstd (put its checkout in \$ZSTD)"
                bad=$((bad + 1))
                continue
            fi
            mkdir -p "$tmp/zstd-obj" || exit 2
            (cd "$tmp/zstd-obj" && ${CC:-cc} -O2 -c -DZSTD_DISABLE_ASM -I "$ZSTD/lib" -I "$ZSTD/lib/common" "$ZSTD"/lib/common/*.c "$ZSTD"/lib/compress/*.c "$ZSTD"/lib/decompress/*.c "$ZSTD"/lib/dictBuilder/*.c) || exit 2
            ${CC:-cc} -O2 -c -I "$ZSTD/doc/educational_decoder" "$here/compare/dump_zstd_edu.c" -o "$tmp/dump_zstd_edu.o" || exit 2
            ${CC:-cc} -O2 -I "$ZSTD/lib" -I "$ZSTD/lib/dictBuilder" "$here/compare/dump_zstd.c" "$tmp/dump_zstd_edu.o" "$tmp"/zstd-obj/*.o -o "$tmp/dump-zstd" || exit 2
            ${CC:-cc} -O2 -I "$ZSTD/lib" "$here/compare/dump_zstd_compress.c" "$tmp"/zstd-obj/*.o -o "$tmp/dump-zstd-compress" || exit 2
            ${CC:-cc} -O2 -I "$ZSTD/lib" "$here/compare/dump_zstd_entropy.c" "$tmp"/zstd-obj/*.o -o "$tmp/dump-zstd-entropy" || exit 2
            built_zstd=1
        fi
        case $what in
        zstd_decode)
            reference=$tmp/dump-zstd
            count=$((blocks / 10))
            extra=$tmp/zstd-frames.bin
            ;;
        zstd_compress)
            reference=$tmp/dump-zstd-compress
            count=$((blocks / 100))
            ;;
        *)
            reference=$tmp/dump-zstd-entropy
            count=$blocks
            ;;
        esac
        ;;
    bc7e_* | rdo_bc7e)
        if [ "$built_ispc" -eq 0 ]; then
            if ! command -v "${ISPC:-ispc}" > /dev/null 2>&1; then
                echo "$what: NOT COMPARED, no ISPC compiler (put ispc on the path or its path in \$ISPC)"
                bad=$((bad + 1))
                continue
            fi
            "${ISPC:-ispc}" -O2 --pic --target=sse2-i32x4 --opt=disable-assertions "$source/bc7e.ispc" -o "$tmp/bc7e.o" -h "$tmp/bc7e_ispc.h" 2> "$tmp/ispc.log" || { cat "$tmp/ispc.log" >&2; exit 2; }
            ${CXX:-c++} -O2 -I "$tmp" "$here/compare/dump_bc7e.cpp" "$tmp/bc7e.o" -o "$tmp/dump-ispc" || exit 2
            # shellcheck disable=SC2086
            ${CXX:-c++} $cpp_flags -DSUPPORT_BC7E=1 -I "$tmp" -I "$source" "$here/compare/dump.cpp" $cpp_sources "$tmp/bc7e.o" -o "$tmp/dump-cpp-bc7e" || exit 2
            built_ispc=1
        fi
        reference=$tmp/dump-ispc
        [ "$what" = rdo_bc7e ] && reference=$tmp/dump-cpp-bc7e
        count=$((blocks / 10))
        ;;
    miniz)
        if [ "$built_miniz" -eq 0 ]; then
            if [ -z "${MINIZ:-}" ] || [ ! -f "$MINIZ/miniz_tdef.c" ]; then
                echo "$what: NOT COMPARED, no miniz (put its checkout in \$MINIZ)"
                bad=$((bad + 1))
                continue
            fi
            # (The header its build would make, and no .zip code, which is not ported.)
            mkdir -p "$tmp/miniz" && echo "#define MINIZ_EXPORT" > "$tmp/miniz/miniz_export.h" || exit 2
            ${CC:-cc} -O2 -DMINIZ_NO_ARCHIVE_APIS -I "$tmp/miniz" -I "$MINIZ" "$here/compare/dump_miniz.c" "$MINIZ/miniz.c" "$MINIZ/miniz_tdef.c" "$MINIZ/miniz_tinfl.c" -o "$tmp/dump-miniz" || exit 2
            built_miniz=1
        fi
        reference=$tmp/dump-miniz
        count=$((blocks / 100))
        ;;
    stb_image)
        if [ "$built_stbi" -eq 0 ]; then
            if [ -z "${STB:-}" ] || [ ! -f "$STB/stb_image.h" ]; then
                echo "$what: NOT COMPARED, no stb (put its checkout in \$STB)"
                bad=$((bad + 1))
                continue
            fi
            if [ -z "${MINIZ:-}" ] || [ ! -f "$MINIZ/miniz_tdef.c" ]; then
                echo "$what: NOT COMPARED, no miniz (put its checkout in \$MINIZ)"
                bad=$((bad + 1))
                continue
            fi
            mkdir -p "$tmp/miniz" && echo "#define MINIZ_EXPORT" > "$tmp/miniz/miniz_export.h" || exit 2
            ${CC:-cc} -O2 -fwrapv -ffp-contract=off -DMINIZ_NO_ARCHIVE_APIS -DMINIZ_NO_ZLIB_COMPATIBLE_NAMES -I "$tmp/miniz" -I "$MINIZ" -I "$STB" "$here/compare/dump_stb_image.c" "$MINIZ/miniz.c" "$MINIZ/miniz_tdef.c" "$MINIZ/miniz_tinfl.c" -lm -o "$tmp/dump-stb-image" || exit 2
            built_stbi=1
        fi
        reference=$tmp/dump-stb-image
        count=$((blocks / 10))
        ;;
    stb_image_write)
        if [ "$built_stbiw" -eq 0 ]; then
            if [ -z "${STB:-}" ] || [ ! -f "$STB/stb_image_write.h" ]; then
                echo "$what: NOT COMPARED, no stb (put its checkout in \$STB)"
                bad=$((bad + 1))
                continue
            fi
            ${CC:-cc} -O2 -ffp-contract=off -I "$STB" "$here/compare/dump_stb_image_write.c" -lm -o "$tmp/dump-stb-image-write" || exit 2
            built_stbiw=1
        fi
        reference=$tmp/dump-stb-image-write
        count=$((blocks / 10))
        ;;
    bc6h_enc)
        if [ "$built_bc6h_enc" -eq 0 ]; then
            if [ -z "${BC6HENC:-}" ] || [ ! -f "$BC6HENC/bc6h_enc.h" ]; then
                echo "$what: NOT COMPARED, no bc6h_enc (put the folder that holds bc6h_enc.h in \$BC6HENC)"
                bad=$((bad + 1))
                continue
            fi
            ${CXX:-c++} -O2 -ffp-contract=off -msse4.1 -mf16c -I "$BC6HENC" "$here/compare/dump_bc6h_enc.cpp" -o "$tmp/dump-bc6h-enc" || exit 2
            built_bc6h_enc=1
        fi
        reference=$tmp/dump-bc6h-enc
        count=$((blocks / 40))
        ;;
    bc6h_pack)
        if [ "$built_bc6h_pack" -eq 0 ]; then
            if [ -z "${BCDEC:-}" ] || [ ! -f "$BCDEC/bcdec.h" ]; then
                echo "$what: NOT COMPARED, no bcdec (put the folder that holds bcdec.h in \$BCDEC)"
                bad=$((bad + 1))
                continue
            fi
            if [ -z "${ISPCTC:-}" ] || [ ! -f "$ISPCTC/ispc_texcomp/kernel.ispc" ]; then
                echo "$what: NOT COMPARED, no ISPC Texture Compressor (put its folder in \$ISPCTC)"
                bad=$((bad + 1))
                continue
            fi
            if ! command -v "${ISPC:-ispc}" > /dev/null 2>&1; then
                echo "$what: NOT COMPARED, no ISPC compiler (put ispc on the path or its path in \$ISPC)"
                bad=$((bad + 1))
                continue
            fi
            "${ISPC:-ispc}" -O2 --pic --target=sse2-i32x4 --opt=disable-assertions "$ISPCTC/ispc_texcomp/kernel.ispc" -o "$tmp/kernel.o" -h "$tmp/kernel_ispc.h" 2> "$tmp/ispc-kernel.log" || { cat "$tmp/ispc-kernel.log" >&2; exit 2; }
            ${CXX:-c++} -O2 -ffp-contract=off -DWITH_ISPC_TEXCOMP=1 -I "$BCDEC" -I "$ISPCTC/ispc_texcomp" -I "$tmp" "$here/compare/dump_bc6h.cpp" "$ISPCTC/ispc_texcomp/ispc_texcomp.cpp" "$tmp/kernel.o" -o "$tmp/dump-bc6h-ispc" || exit 2
            built_bc6h_pack=1
        fi
        reference=$tmp/dump-bc6h-ispc
        count=$((blocks / 10))
        ;;
    bc2)
        if [ "$built_bc2" -eq 0 ]; then
            if [ -z "${BCDEC:-}" ] || [ ! -f "$BCDEC/bcdec.h" ]; then
                echo "$what: NOT COMPARED, no bcdec (put the folder that holds bcdec.h in \$BCDEC)"
                bad=$((bad + 1))
                continue
            fi
            # shellcheck disable=SC2086
            ${CXX:-c++} $cpp_flags -DWITH_BCDEC=1 -I "$BCDEC" -I "$source" "$here/compare/dump.cpp" $cpp_sources -o "$tmp/dump-cpp-bc2" || exit 2
            built_bc2=1
        fi
        reference=$tmp/dump-cpp-bc2
        count=$blocks
        ;;
    bc6h_*)
        if [ "$built_bc6h" -eq 0 ]; then
            if [ -z "${BCDEC:-}" ] || [ ! -f "$BCDEC/bcdec.h" ]; then
                echo "$what: NOT COMPARED, no bcdec (put the folder that holds bcdec.h in \$BCDEC)"
                bad=$((bad + 1))
                continue
            fi
            ${CXX:-c++} -O2 -ffp-contract=off -I "$BCDEC" "$here/compare/dump_bc6h.cpp" -o "$tmp/dump-bc6h" || exit 2
            built_bc6h=1
        fi
        reference=$tmp/dump-bc6h
        count=$blocks
        ;;
    *)
        if [ "$built_cpp" -eq 0 ]; then
            # shellcheck disable=SC2086
            ${CXX:-c++} $cpp_flags -I "$source" "$here/compare/dump.cpp" $cpp_sources -o "$tmp/dump-cpp" || exit 2
            built_cpp=1
        fi
        reference=$tmp/dump-cpp
        count=$blocks
        case $what in ert | rdo) count=$((blocks / 10)) ;; esac
        ;;
    esac
    seed=1
    while [ "$seed" -le "$seeds" ]; do
        # shellcheck disable=SC2086
        "$reference" "$what" "$seed" "$count" $extra > "$tmp/reference.txt" 2> /dev/null
        # shellcheck disable=SC2086
        "$rust" "$what" "$seed" "$count" $extra > "$tmp/rust.txt"
        if cmp -s "$tmp/reference.txt" "$tmp/rust.txt"; then
            echo "$what, seed $seed: the same ($(tail -1 "$tmp/rust.txt"))"
        else
            echo "$what, seed $seed: DIFFERENT"
            bad=$((bad + 1))
        fi
        seed=$((seed + 1))
    done
done
[ "$bad" -eq 0 ]
