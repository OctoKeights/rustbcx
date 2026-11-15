#!/bin/sh
# Checks the files the crate writes against other readers.
#
#   KTX=<checkout of KTX-Software> KTX_BUILD=<its build folder> TINYDDS=<checkout of tinyddsloader> \
#       [FFMPEG=<ffmpeg>] compare/files.sh
#
# examples/files.rs writes textures of every format (BC1 to BC7) and shape (plain, texture
# array, array of one, cube map, array of cube maps, volume texture), with one level and with
# all of them, as .dds, .ktx and .ktx2, and prints for each what a reader should find: its size,
# layers, faces and levels, and a hash of every image of every level. Then:
#
# - Every .ktx2 file goes through the Khronos validator (`ktx validate`, warnings as errors),
#   those with ZLIB and with Zstandard supercompression (.zlib.ktx2, .zstd.ktx2) among them.
# - Every .ktx and .ktx2 file is read by the Khronos library (compare/files_ktx.c, built
#   against libktx), which has to find the same; from a supercompressed file, after
#   decompressing it with its own zlib or zstd.
# - Every .ktx2 file is compressed again by the Khronos tool (`ktx deflate --zstd`, at levels
#   from 1 to 19), and what it writes is read by the crate, which has to find the same.
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
# And the .exr files: the same example writes made-up images in scanlines and in tiles, as
# halves and as floats, with each of the eight compressions, and beside each the floats the
# crate reads of it. Every file is then read by ffmpeg (`ffmpeg` on the path, or its path in
# $FFMPEG), whose .exr reader is its own and not OpenEXR's or tinyexr's, and which has to find
# the same floats, bit for bit: for B44 and for PXR24's floats too, which lose a little, the
# same on both sides. The other way round, ffmpeg writes an image again in each of the ways
# it can (uncompressed, RLE and ZIP by the line and by sixteen lines, as halves and as floats),
# and the crate has to read the same out of what it writes.
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
    # shellcheck disable=SC2086
    "$tmp/files-ktx" "$tmp/out" zstd.ktx2 $names > "$tmp/zstd-ktx2.txt" 2>&1
    same ".ktx2 with Zstandard supercompression by the Khronos library" "$tmp/zstd-ktx2.txt"
    if [ -x "$lib/ktx" ]; then
        level=0
        for name in $names; do
            level=$((level % 19 + 1))
            "$lib/ktx" deflate --zstd "$level" "$tmp/out/$name.ktx2" "$tmp/out/$name.khronos-zstd.ktx2" > /dev/null 2>&1
        done
        "$rust" "$tmp/out" khronos-zstd.ktx2 > "$tmp/khronos-zstd.txt" 2>&1
        same ".ktx2 with Zstandard supercompression from the Khronos tool, by the crate" "$tmp/khronos-zstd.txt"
    else
        echo ".ktx2 with Zstandard supercompression from the Khronos tool: NOT COMPARED, no ktx tool in $lib"
        bad=$((bad + 1))
    fi
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
ffmpeg=${FFMPEG:-ffmpeg}
if ! command -v "$ffmpeg" > /dev/null 2>&1; then
    echo ".exr: NOT COMPARED, no ffmpeg (on the path, or its path in \$FFMPEG)"
    bad=$((bad + 1))
else
    mkdir -p "$tmp/exr" || exit 2
    "$rust" --exr "$tmp/exr" || exit 2
    other=0
    for file in "$tmp/exr"/*.exr; do
        if ! "$ffmpeg" -v error -y -i "$file" -f rawvideo -pix_fmt gbrpf32le "$tmp/exr/read.raw" > /dev/null 2>&1 || ! cmp -s "$tmp/exr/read.raw" "$file.gbr"; then
            [ "$other" -eq 0 ] && echo "$file: ffmpeg does not read what the crate reads"
            other=$((other + 1))
        fi
    done
    if [ "$other" -eq 0 ]; then
        echo ".exr by ffmpeg: the same in all $(ls "$tmp/exr"/*.exr | wc -l | tr -d ' ') files"
    else
        echo ".exr by ffmpeg: $other DIFFERENT"
        bad=$((bad + 1))
    fi
    # The other way round: ffmpeg writes the image again, each of the four ways it can
    # compress and as halves and as floats, and the crate has to read the same floats.
    other=0
    count=0
    for compression in none rle zip1 zip16; do
        for stored in half float; do
            count=$((count + 1))
            if ! "$ffmpeg" -v error -y -i "$tmp/exr/lines-none-$stored.exr" -c:v exr -compression "$compression" -format "$stored" "$tmp/exr/from-ffmpeg.exr" > /dev/null 2>&1 || ! "$rust" --exr-same "$tmp/exr/from-ffmpeg.exr" "$tmp/exr/lines-none-$stored.exr.gbr"; then
                [ "$other" -eq 0 ] && echo "$compression, $stored: the crate does not read what ffmpeg wrote"
                other=$((other + 1))
            fi
        done
    done
    if [ "$other" -eq 0 ]; then
        echo ".exr from ffmpeg, by the crate: the same in all $count files"
    else
        echo ".exr from ffmpeg, by the crate: $other DIFFERENT"
        bad=$((bad + 1))
    fi
fi
[ "$bad" -eq 0 ]
