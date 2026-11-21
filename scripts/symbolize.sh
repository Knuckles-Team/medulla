#!/usr/bin/env bash
# Resolve stack offsets of a released server to functions and source lines
# (MEDULLA-CONTRACT-R060.3).
#
#   symbolize.sh [--artifact-dir <dir>] --build-id <id> <tag> <platform> <offset>...
#
# <tag> is the release tag, <platform> the release matrix name (linux-aarch64,
# windows-x86_64, macos-aarch64) and <id> the build-id the running server printed
# with `--build-id`. The script takes the symbol artifact
# symbols-<platform>-<id>.tar.zst of the tag (downloaded with `gh release download`,
# or read from --artifact-dir when given), verifies its sha256, checks that the
# symbol file inside carries the build-id of the running server, and resolves each
# offset with addr2line. It prints one line per offset: "<offset> <function> <file:line>".
# Writes: a temporary directory, removed on exit.
set -euo pipefail

usage() {
    echo "usage: $0 [--artifact-dir <dir>] --build-id <id> <tag> <platform> <offset>..." >&2
    exit 2
}

artifact_dir=""
build_id=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artifact-dir) [ "$#" -ge 2 ] || usage; artifact_dir="$2"; shift 2 ;;
        --build-id) [ "$#" -ge 2 ] || usage; build_id="$2"; shift 2 ;;
        --) shift; break ;;
        -*) usage ;;
        *) break ;;
    esac
done
[ -n "$build_id" ] || usage
[ "$#" -ge 3 ] || usage
tag="$1"
platform="$2"
shift 2

here="$(dirname "${BASH_SOURCE[0]}")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
name="symbols-$platform-$build_id.tar.zst"

if [ -n "$artifact_dir" ]; then
    source_dir="$artifact_dir"
else
    gh release download "$tag" --pattern "$name" --pattern "$name.sha256" --dir "$work/download"
    source_dir="$work/download"
fi
[ -f "$source_dir/$name" ] || { echo "SYMBOL_ARTIFACT_MISSING: $name" >&2; exit 1; }
[ -f "$source_dir/$name.sha256" ] || { echo "SYMBOL_ARTIFACT_MISSING: $name.sha256" >&2; exit 1; }
(cd "$source_dir" && sha256sum --check --status "$name.sha256") \
    || { echo "SYMBOL_ARTIFACT_DIGEST_MISMATCH: $name" >&2; exit 1; }

mkdir "$work/symbols"
zstd -d -q -c "$source_dir/$name" | tar -xf - -C "$work/symbols"
symbol_file="$(find "$work/symbols" -type f -name '*.debug' | sort | head -n 1)"
[ -n "$symbol_file" ] || { echo "SYMBOL_FILE_MISSING: $name holds no .debug symbol file" >&2; exit 1; }

found_id="$(python3 "$here/read_build_id.py" "$symbol_file" 2>/dev/null || true)"
if [ "$found_id" != "$build_id" ]; then
    echo "BUILD_ID_MISMATCH: the symbol file carries '${found_id:-none}', the server printed '$build_id'" >&2
    exit 1
fi

for offset in "$@"; do
    {
        read -r function
        read -r location
    } < <(addr2line -f -C -e "$symbol_file" "$offset")
    echo "$offset $function $location"
done
