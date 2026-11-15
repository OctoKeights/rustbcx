#!/bin/sh
# Checks the files the crate writes against other readers.
#
#   KTX=<checkout of KTX-Software> KTX_BUILD=<its build folder> TINYDDS=<checkout of tinyddsloader> \
#       compare/files.sh
#
# examples/files.rs writes textures of every format (BC1 to BC7) and shape (plain, texture
# array, array of one, cube map, array of cube maps, volume texture), with one level and with
# all of them, as .dds, .ktx and .ktx2, and prints for each what a reader should find: its size,
# layers, faces and levels, and a hash of every image of every level. Then:
#
# - Every .ktx2 file goes through the Khronos validator (`ktx validate`, warnings as errors),
#   those with ZLIB supercompression (.zlib.ktx2) among them.
# - Every .ktx and .ktx2 file is read by the Khronos library (compare/files_ktx.c, built
#   against libktx), which has to find the same; from a supercompressed file, after
#   decompressing it with its own zlib.
# - Every .dds file is read by tinyddsloader (compare/files_dds.cpp), which has to find the
#   same.
#
# KTX-Software is https://github.com/KhronosGroup/KTX-Software; $KTX_BUILD is where it was
# built (cmake, then the target `ktxtools`), and holds the `ktx` tool and libktx, in its
# Release folder or by themselves. tinyddsloader is https://github.com/benikabocha/tinyddsloader.
# It walks a file as Microsoft's DDSTextureLoader does, but never takes a volume's depth from
# the header, so the script builds against a copy with that put in (two lines; without them
# it finds one slice in every volume).
#
# What cannot be built is said to be NOT COMPARED, and the script fails.
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
tmp=${TMPDIR:-/tmp}/rustbcx-files
rm -rf "$tmp"
mkdir -p "$tmp/out" "$tmp/tinydds" || exit 2
bad=0

(cd "$here" && cargo build --release -q --example files) || exit 2
rust=$(cd "$here" && cargo metadata --format-version 1 --no-deps 2>/dev/null | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')/release/examples/files
"$rust" "$tmp/out" > "$tmp/expected.txt" || exit 2
names=$(grep " layers " "$tmp/expected.txt" | cut -d' ' -f1 | tr '\n' ' ')
echo "written and read back by the crate: $(ls "$tmp/out" | wc -l | tr -d ' ') files"

same() {
    if cmp -s "$tmp/expected.txt" "$2"; then
        echo "$1: the same"
    else
        echo "$1: DIFFERENT"
        bad=$((bad + 1))
    fi
}

lib=
if [ -n "${KTX_BUILD:-}" ]; then
    for folder in "$KTX_BUILD/Release" "$KTX_BUILD"; do
        if ls "$folder"/libktx.* > /dev/null 2>&1; then lib=$folder; break; fi
    done
fi
if [ -z "${KTX:-}" ] || [ ! -f "$KTX/lib/include/ktx.h" ] || [ -z "$lib" ]; then
    echo ".ktx and .ktx2: NOT COMPARED, no KTX-Software (its checkout in \$KTX, its build folder in \$KTX_BUILD)"
    bad=$((bad + 1))
else
    LD_LIBRARY_PATH=$lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
    export LD_LIBRARY_PATH
    if [ -x "$lib/ktx" ]; then
        invalid=0
        for file in "$tmp/out"/*.ktx2; do
            if ! "$lib/ktx" validate --warnings-as-errors "$file" > "$tmp/validate.txt" 2>&1; then
                [ "$invalid" -eq 0 ] && { echo "$file:"; cat "$tmp/validate.txt"; }
                invalid=$((invalid + 1))
            fi
        done
        if [ "$invalid" -eq 0 ]; then
            echo ".ktx2 by the Khronos validator: all $(ls "$tmp/out"/*.ktx2 | wc -l | tr -d ' ') valid"
        else
            echo ".ktx2 by the Khronos validator: $invalid NOT VALID"
            bad=$((bad + 1))
        fi
    else
        echo ".ktx2 by the Khronos validator: NOT COMPARED, no ktx tool in $lib"
        bad=$((bad + 1))
    fi
    ${CC:-cc} -O1 -I "$KTX/lib/include" -I "$KTX/external/dfdutils" "$here/compare/files_ktx.c" -L "$lib" -lktx -o "$tmp/files-ktx" || exit 2
    # shellcheck disable=SC2086
    "$tmp/files-ktx" "$tmp/out" ktx $names > "$tmp/ktx.txt" 2>&1
    same ".ktx by the Khronos library" "$tmp/ktx.txt"
    # shellcheck disable=SC2086
    "$tmp/files-ktx" "$tmp/out" ktx2 $names > "$tmp/ktx2.txt" 2>&1
    same ".ktx2 by the Khronos library" "$tmp/ktx2.txt"
    # shellcheck disable=SC2086
    "$tmp/files-ktx" "$tmp/out" zlib.ktx2 $names > "$tmp/zlib-ktx2.txt" 2>&1
    same ".ktx2 with ZLIB supercompression by the Khronos library" "$tmp/zlib-ktx2.txt"
fi

if [ -z "${TINYDDS:-}" ] || [ ! -f "$TINYDDS/tinyddsloader.h" ]; then
    echo ".dds: NOT COMPARED, no tinyddsloader (its checkout in \$TINYDDS)"
    bad=$((bad + 1))
else
    sed -e 's|            m_texDim = TextureDimension::Texture3D;|            m_texDim = TextureDimension::Texture3D; m_depth = header->m_depth;|' \
        -e 's|                if (m_arraySize > 1) {|                m_depth = header->m_depth; if (m_arraySize > 1) {|' \
        "$TINYDDS/tinyddsloader.h" > "$tmp/tinydds/tinyddsloader.h"
    if [ "$(grep -c 'm_depth = header->m_depth' "$tmp/tinydds/tinyddsloader.h")" -ne 2 ]; then
        echo ".dds: NOT COMPARED, tinyddsloader.h is not the one this script knows"
        bad=$((bad + 1))
    else
        ${CXX:-c++} -O1 -w -include cstdint -I "$tmp/tinydds" "$here/compare/files_dds.cpp" -o "$tmp/files-dds" || exit 2
        # shellcheck disable=SC2086
        "$tmp/files-dds" "$tmp/out" $names > "$tmp/dds.txt" 2>&1
        same ".dds by tinyddsloader" "$tmp/dds.txt"
    fi
fi
[ "$bad" -eq 0 ]
