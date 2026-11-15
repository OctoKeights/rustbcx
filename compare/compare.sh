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
# Without the ISPC compiler the bc7e dumps and bc6h_pack are left out, without $BCDEC the BC6H
# dumps, and without $ISPCTC bc6h_pack; the script says so and fails.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <folder with bc7enc_rdo's sources> [blocks] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-bc7decomp unpack bc1 bc345 bc7enc bc7block ert rdo bc7e_presets bc7e_settings rdo_bc7e bc6h_unpack bc6h_pack}
tmp=${TMPDIR:-/tmp}/rustbcx-compare
mkdir -p "$tmp" || exit 2

(cd "$here" && cargo build --release -q --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

built_cpp=0
built_ispc=0
built_bc6h=0
built_bc6h_pack=0
bad=0
# (ert.h uses the fixed-width integer types without including their header; no contraction of
# a multiply and an add into one instruction, which Rust never does.)
cpp_flags="-O2 -ffp-contract=off -include stdint.h"
cpp_sources="$source/bc7decomp.cpp $source/bc7decomp_ref.cpp $source/bc7enc.cpp $source/rgbcx.cpp $source/ert.cpp $source/rdo_bc_encoder.cpp $source/utils.cpp $source/lodepng.cpp"
for what in $dumps; do
    case $what in
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
        "$reference" "$what" "$seed" "$count" > "$tmp/reference.txt" 2> /dev/null
        "$rust" "$what" "$seed" "$count" > "$tmp/rust.txt"
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
