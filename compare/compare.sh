#!/bin/sh
# Compares the Rust port with the originals: bc7enc_rdo's C++ library and bc7e.ispc.
#
#   compare/compare.sh <folder with bc7enc_rdo's sources> [blocks] [seeds] [dumps]
#
# The folder is a checkout of https://github.com/richgel999/bc7enc_rdo: it holds both
# originals. Runs every dump of both sides for each seed and compares what they print byte for
# byte. [dumps] names the dumps to run, in quotes, when not all of them are wanted.
#
# The C++ library (bc7decomp.cpp, bc7enc.cpp, rgbcx.cpp): compare/dump.cpp is built against
# it. Dumps: bc7decomp unpack bc1 bc345 bc7enc bc7block.
#
# bc7e.ispc: needs the ISPC compiler (https://github.com/ispc/ispc/releases), `ispc` on the
# path or its path in $ISPC. bc7e.ispc is built the strict way (for SSE2, without fast-math:
# no fused multiply-add, no reordered arithmetic), which gives the same blocks on every
# machine, and compare/dump_bc7e.cpp is built against it. Dumps: bc7e_presets bc7e_settings,
# each with a tenth of [blocks], since it packs every block fourteen times or with thorough
# settings. (A build of bc7e.ispc with --opt=fast-math, or for AVX2 without
# --opt=disable-fma, packs some blocks differently from this one, and so from the port.)
#
# Without the ISPC compiler the bc7e dumps are left out, and the script says so and fails.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 <folder with bc7enc_rdo's sources> [blocks] [seeds] [dumps]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
source=$1
blocks=${2:-200000}
seeds=${3:-3}
dumps=${4:-bc7decomp unpack bc1 bc345 bc7enc bc7block bc7e_presets bc7e_settings}
tmp=${TMPDIR:-/tmp}/rustbcx-compare
mkdir -p "$tmp" || exit 2

(cd "$here" && cargo build --release -q --example dump) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/dump

built_cpp=0
built_ispc=0
bad=0
for what in $dumps; do
    case $what in
    bc7e_*)
        if [ "$built_ispc" -eq 0 ]; then
            if ! command -v "${ISPC:-ispc}" > /dev/null 2>&1; then
                echo "$what: NOT COMPARED, no ISPC compiler (put ispc on the path or its path in \$ISPC)"
                bad=$((bad + 1))
                continue
            fi
            "${ISPC:-ispc}" -O2 --pic --target=sse2-i32x4 --opt=disable-assertions "$source/bc7e.ispc" -o "$tmp/bc7e.o" -h "$tmp/bc7e_ispc.h" 2> "$tmp/ispc.log" || { cat "$tmp/ispc.log" >&2; exit 2; }
            ${CXX:-c++} -O2 -I "$tmp" "$here/compare/dump_bc7e.cpp" "$tmp/bc7e.o" -o "$tmp/dump-ispc" || exit 2
            built_ispc=1
        fi
        reference=$tmp/dump-ispc
        count=$((blocks / 10))
        ;;
    *)
        if [ "$built_cpp" -eq 0 ]; then
            # (No contraction of a multiply and an add into one instruction: Rust never does it.)
            ${CXX:-c++} -O2 -ffp-contract=off -I "$source" "$here/compare/dump.cpp" "$source/bc7decomp.cpp" "$source/bc7enc.cpp" "$source/rgbcx.cpp" -o "$tmp/dump-cpp" || exit 2
            built_cpp=1
        fi
        reference=$tmp/dump-cpp
        count=$blocks
        ;;
    esac
    seed=1
    while [ "$seed" -le "$seeds" ]; do
        "$reference" "$what" "$seed" "$count" > "$tmp/reference.txt"
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
