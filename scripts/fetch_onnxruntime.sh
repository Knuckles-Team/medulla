#!/usr/bin/env bash
# Fetch the ONNX Runtime shared library that medulla-tts-piper's synthesis tests load
# under `ort-load-dynamic` (on under `--all-features`), and print its path.
#
#   export ORT_DYLIB_PATH="$(scripts/fetch_onnxruntime.sh)"
#   cargo test -p medulla-tts-piper --all-features
#
# The version matches the ONNX Runtime release `ort-sys` 2.0.0-rc.12 pins
# (api-24). An older library such as Debian's `libonnxruntime1.23` cannot
# serve that API version. The library is taken from the official onnxruntime
# PyPI wheel (the Microsoft GitHub release archive for this version is no longer
# served: the download answers 404) and the wheel is checked against a pinned
# sha256 before use. Only the library is extracted; the wheel's Python code is
# never imported or run. The wheel is built for the manylinux_2_28 baseline and
# onnxruntime picks its kernels at run time from the CPU's features; whether it
# loads and runs these tests on a host without AVX2 is verified by the
# tts-piper-inference job on a no-AVX2 runner, not assumed here.
#
# Optional argument: cache directory (default: $XDG_CACHE_HOME/medulla-onnxruntime).
set -euo pipefail

VERSION=1.24.2
WHEEL=onnxruntime-$VERSION-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl
WHEEL_URL="https://files.pythonhosted.org/packages/35/af/ad86cfbfd65d5a86204b3a30893e92c0cf3f1a56280efc5a12e69d81f52d/$WHEEL"
WHEEL_SHA256=09aa6f8d766b4afc3cfba68dd10be39586b49f9462fbd1386c5d5644239461ca

if [ "$(uname -s)-$(uname -m)" != "Linux-x86_64" ]; then
  echo "fetch_onnxruntime.sh: only linux x86_64 is pinned; got $(uname -s)-$(uname -m)" >&2
  exit 1
fi

dest="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/medulla-onnxruntime}"
lib="$dest/$VERSION/libonnxruntime.so.$VERSION"

if [ ! -f "$lib" ]; then
  mkdir -p "$dest/$VERSION"
  wheel="$dest/$WHEEL"
  curl -fsSL --retry 3 -o "$wheel.part" "$WHEEL_URL"
  echo "$WHEEL_SHA256  $wheel.part" | sha256sum --check --quiet -
  mv "$wheel.part" "$wheel"
  python3 -I - "$wheel" "$dest/$VERSION" "libonnxruntime.so.$VERSION" <<'PY'
import sys
import zipfile

wheel, out, name = sys.argv[1:4]
with zipfile.ZipFile(wheel) as z:
    data = z.read(f"onnxruntime/capi/{name}")
with open(f"{out}/{name}.part", "wb") as f:
    f.write(data)
PY
  chmod 0755 "$lib.part"
  mv "$lib.part" "$lib"
fi

test -f "$lib"
printf '%s\n' "$lib"
