#!/usr/bin/env bash
# Package the split debug symbols of one release platform (MEDULLA-CONTRACT-R060.2).
#
#   package_release_symbols.sh <platform> <binary> <symbols-dir> <out-dir>
#
# Writes <out-dir>/symbols-<platform>-<build-id>.tar.zst and its .sha256. The build-id
# is read from the shipped <binary> by scripts/read_build_id.py. The archive is a tar
# of <symbols-dir> with sorted names, mtime SOURCE_DATE_EPOCH and owner 0, compressed
# with `zstd -19`, so one input yields one archive.
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <platform> <binary> <symbols-dir> <out-dir>" >&2
    exit 2
fi
platform="$1"
binary="$2"
symbols="$3"
out="$4"
[ -f "$binary" ] || { echo "no such binary: $binary" >&2; exit 1; }
[ -d "$symbols" ] || { echo "no such symbols directory: $symbols" >&2; exit 1; }
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH must be set}"

here="$(dirname "${BASH_SOURCE[0]}")"
build_id="$(python3 "$here/read_build_id.py" "$binary")"
mkdir -p "$out"
archive="$out/symbols-$platform-$build_id.tar.zst"
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$symbols" -cf - . | zstd -19 -q -f -o "$archive"
(cd "$out" && sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256")
echo "symbols: $archive"
