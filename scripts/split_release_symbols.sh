#!/usr/bin/env bash
# Split the debug information from a release executable (MEDULLA-CONTRACT-R060.1).
#
#   split_release_symbols.sh <binary> <outdir>
#
# Writes the symbol file into <outdir> and strips <binary> in place, so exactly two
# files remain: the shipped executable and its symbol file. The build identity stays
# in both: the GNU build-id on Linux (the link passes `-Wl,--build-id=sha1`), the
# Mach-O UUID on macOS, the PE debug GUID on Windows. The output is a pure function
# of the input bytes, so two builds that produce the same executable produce the same
# symbol file.
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <binary> <outdir>" >&2
    exit 2
fi
binary="$1"
outdir="$2"
[ -f "$binary" ] || { echo "no such binary: $binary" >&2; exit 1; }
mkdir -p "$outdir"
name="$(basename "$binary")"

case "$(uname -s)" in
    Linux)
        debug="$outdir/$name.debug"
        objcopy --only-keep-debug "$binary" "$debug"
        objcopy --strip-debug --add-gnu-debuglink="$debug" "$binary"
        ;;
    Darwin)
        debug="$outdir/$name.dSYM"
        rm -rf "$debug"
        dsymutil "$binary" -o "$debug"
        strip -S "$binary"
        ;;
    MINGW* | MSYS* | CYGWIN*)
        # The linker writes the .pdb next to the executable; its GUID is the
        # executable's PE debug GUID. The executable already carries no debug data.
        pdb="${binary%.exe}.pdb"
        [ -f "$pdb" ] || { echo "no .pdb next to $binary" >&2; exit 1; }
        debug="$outdir/$(basename "$pdb")"
        cp "$pdb" "$debug"
        ;;
    *)
        echo "unsupported platform: $(uname -s)" >&2
        exit 1
        ;;
esac
echo "symbols: $debug"
