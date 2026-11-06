#!/usr/bin/env python3
"""Build the eg2. method-body codec module the Go and JavaScript clients embed.

The engine MACs the canonical body it re-derives from the request it decoded.
``crates/medulla-method-codec`` exports that exact decoder and encoder as a
WebAssembly module with no imports; this script builds it for
``wasm32-unknown-unknown`` under the ``method-codec`` profile, with the same
identity-neutral path remaps the release wheels use, and places one copy in
each client directory (Go's ``//go:embed`` cannot reach outside its module).

The module is never committed (MEDULLA-SDLC-R020.1): it is built where it is
consumed -- the language-clients CI job runs this script before ``go test`` and
``npm test``, and a Go or JavaScript release packs the bytes this script writes.
Both client copies are gitignored. Needs the pinned toolchain's
``wasm32-unknown-unknown`` target (``rustup target add wasm32-unknown-unknown``).

The Rust ``method-codec`` profile supplies the module bytes directly. The
builder verifies the WebAssembly header and refuses an import section. Two
independent canonical builds must be byte-identical and within ``MAX_BYTES``
(MEDULLA-CONTRACT-R006.2) before any copy is written.
The linked module is then shrunk by binaryen ``wasm-opt -Oz`` from one pinned
release archive (``BINARYEN_URL``), fetched into the cargo target directory and
refused unless its SHA-256 equals ``BINARYEN_SHA256``; the same pinned binary
gives the same bytes on every host.

``--export-dir`` saves the fresh module and a SHA-256/source/build provenance
sidecar. Failed builds clear old exports and never advertise a completed export.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import tarfile
import urllib.request
from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from configure_rust_path_remap import encoded_rustflags
else:
    try:
        from configure_rust_path_remap import encoded_rustflags
    except ModuleNotFoundError:  # imported as a package in tests
        from scripts.configure_rust_path_remap import encoded_rustflags

ROOT = Path(__file__).resolve().parents[1]
TARGET = "wasm32-unknown-unknown"
PROFILE = "method-codec"
MODULE = "medulla_method_codec.wasm"
CLIENT_DIRS = ("clients/go", "clients/js")
PROVENANCE = "medulla_method_codec.provenance.json"
# Fixed release gate (MEDULLA-CONTRACT-R006): lowered, never raised.
MAX_BYTES = 10_946_464
CANONICAL_BUILDS = 2
BUILD_COMMAND = (
    "cargo",
    "build",
    "--locked",
    "-p",
    "medulla-method-codec",
    "--lib",
    "--profile",
    PROFILE,
    "--target",
    TARGET,
)


BINARYEN_VERSION = "version_132"
BINARYEN_URL = (
    "https://github.com/WebAssembly/binaryen/releases/download/"
    f"{BINARYEN_VERSION}/binaryen-{BINARYEN_VERSION}-x86_64-linux.tar.gz"
)
BINARYEN_SHA256 = "195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572"
# The features rustc's wasm32-unknown-unknown target enables by default; wasm-opt
# must accept every instruction the linker emitted.
WASM_FEATURES = (
    "--enable-bulk-memory",
    "--enable-sign-ext",
    "--enable-mutable-globals",
    "--enable-nontrapping-float-to-int",
    "--enable-multivalue",
    "--enable-reference-types",
)
WASM_OPT_PASSES = ("-Oz", "--strip-debug", "--strip-producers")


def wasm_opt(target_dir: Path) -> Path:
    """Return the pinned wasm-opt, fetching and verifying its archive once."""

    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise RuntimeError("the pinned binaryen archive is x86_64 Linux only")
    home = target_dir / f"binaryen-{BINARYEN_VERSION}"
    # The release archive unpacks to ``binaryen-<version>/bin/wasm-opt``.
    binary = home / f"binaryen-{BINARYEN_VERSION}" / "bin" / "wasm-opt"
    if binary.is_file():
        return binary
    home.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(BINARYEN_URL, timeout=300) as response:
        archive = response.read()
    if _sha256(archive) != BINARYEN_SHA256:
        raise RuntimeError(
            f"binaryen archive sha256={_sha256(archive)} != {BINARYEN_SHA256}"
        )
    tarball = home / "binaryen.tar.gz"
    tarball.write_bytes(archive)
    with tarfile.open(tarball) as bundle:
        bundle.extractall(home, filter="data")
    return binary


def optimize(module: Path, target_dir: Path) -> bytes:
    """Shrink the linked module with the pinned ``wasm-opt -Oz``."""

    optimized = module.with_suffix(".opt.wasm")
    command = [str(wasm_opt(target_dir)), *WASM_FEATURES, *WASM_OPT_PASSES]
    subprocess.run([*command, str(module), "-o", str(optimized)], check=True)
    return optimized.read_bytes()


def build(root: Path, target_dir: Path) -> bytes:
    """Compile the Rust codec and return its import-free WebAssembly bytes."""

    # Hermetic flags: ambient RUSTFLAGS would change the module per host. The
    # Cargo and rustup homes are named explicitly (their defaults) so their
    # remaps apply even where the variables are unset -- registry crates'
    # source paths survive into panic locations otherwise.
    env = {
        name: value
        for name, value in os.environ.items()
        if name not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")
    }
    env.setdefault("CARGO_HOME", str(Path.home() / ".cargo"))
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    env["CARGO_TARGET_DIR"] = str(target_dir)
    flags, _, _ = encoded_rustflags(env, checkout=root, target=TARGET)
    env["CARGO_ENCODED_RUSTFLAGS"] = flags
    subprocess.run(list(BUILD_COMMAND), cwd=root, env=env, check=True)
    module = optimize(target_dir / TARGET / PROFILE / MODULE, target_dir)
    verify_no_imports(module)
    return module


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_size(module: bytes, offset: int) -> tuple[int, int]:
    """Read a bounded WebAssembly section size (unsigned LEB128 u32)."""

    value = 0
    for shift in range(0, 35, 7):
        if offset >= len(module):
            raise ValueError("truncated WebAssembly section size")
        byte = module[offset]
        offset += 1
        if shift == 28 and byte > 0x0F:
            raise ValueError("invalid WebAssembly section size")
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, offset
    raise ValueError("invalid WebAssembly section size")


def verify_no_imports(module: bytes) -> None:
    """Fail closed if the linked module has imports or malformed sections."""

    if not module.startswith(b"\x00asm\x01\x00\x00\x00"):
        raise ValueError("invalid WebAssembly magic or version")
    offset = 8
    while offset < len(module):
        section_id = module[offset]
        size, body_start = _read_size(module, offset + 1)
        offset = body_start + size
        if offset > len(module):
            raise ValueError("truncated WebAssembly section")
        if section_id == 2:
            raise ValueError("method codec must have zero WebAssembly imports")


def write(root: Path, module: bytes, rebuild: bytes | None = None) -> int:
    """Qualify two canonical builds, then place the module in every client."""

    verify_no_imports(module)
    if rebuild is not None and rebuild != module:
        print(
            f"REFUSED: two canonical builds differ: sha256={_sha256(module)} "
            f"vs sha256={_sha256(rebuild)}",
            file=sys.stderr,
        )
        return 1
    if len(module) > MAX_BYTES:
        print(
            f"REFUSED: {MODULE} is {len(module)} bytes, over MAX_BYTES={MAX_BYTES}",
            file=sys.stderr,
        )
        return 1
    for client in CLIENT_DIRS:
        if (root / client / MODULE).is_symlink():
            print(f"REFUSED: {client}/{MODULE} must not be a symlink", file=sys.stderr)
            return 1
    for client in CLIENT_DIRS:
        (root / client / MODULE).write_bytes(module)
    clients = ", ".join(CLIENT_DIRS)
    print(f"wrote {MODULE} sha256={_sha256(module)} ({len(module)} bytes) to {clients}")
    return 0


def export(root: Path, directory: Path, module: bytes) -> None:
    """Export verified fresh bytes with source and canonical recipe provenance."""

    verify_no_imports(module)

    def git(*args: str) -> str:
        return subprocess.check_output(["git", *args], cwd=root, text=True).strip()

    provenance = {
        "sha256": _sha256(module),
        "size_bytes": len(module),
        "source_revision": git("rev-parse", "HEAD"),
        "source_tree": git("rev-parse", "HEAD^{tree}"),
        "source_dirty": bool(git("status", "--porcelain", "--untracked-files=no")),
        "build_command": list(BUILD_COMMAND),
        "target": TARGET,
        "profile": PROFILE,
        "rustc_version": subprocess.check_output(
            ["rustc", "--version", "--verbose"], cwd=root, text=True
        ).strip(),
        "recipe_sha256": {
            name: _sha256((root / name).read_bytes())
            for name in (
                "Cargo.lock",
                "Cargo.toml",
                "rust-toolchain.toml",
                "scripts/build_method_codec_wasm.py",
                "scripts/configure_rust_path_remap.py",
            )
        },
    }
    directory.mkdir(parents=True, exist_ok=True)
    (directory / MODULE).write_bytes(module)
    (directory / PROVENANCE).write_text(
        json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    # This step-scoped signal cannot be supplied by a restored target cache.
    if output := os.environ.get("GITHUB_OUTPUT"):
        with Path(output).open("a", encoding="utf-8") as stream:
            stream.write("fresh_export=true\n")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=None,
        help="cargo target directory (default: $CARGO_TARGET_DIR, else <repo>/target)",
    )
    parser.add_argument(
        "--export-dir",
        type=Path,
        help="also export the fresh WASM and its provenance to this directory",
    )
    args = parser.parse_args(argv)
    if args.export_dir:
        # Clear only our two outputs, before attempting a new build.
        for name in (MODULE, PROVENANCE):
            (args.export_dir / name).unlink(missing_ok=True)
    target_dir = args.target_dir or Path(
        os.environ.get("CARGO_TARGET_DIR", ROOT / "target")
    )
    module = build(ROOT, target_dir.resolve())
    # The second canonical build is independent: its own fresh target directory.
    rebuild = build(ROOT, (target_dir / "method-codec-requalify").resolve())
    if args.export_dir:
        export(ROOT, args.export_dir, module)
    return write(ROOT, module, rebuild)


if __name__ == "__main__":
    raise SystemExit(main())
