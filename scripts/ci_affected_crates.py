#!/usr/bin/env python3
"""Select the workspace crates a pull request affects and print the cargo runs.

The pull-request ``CI`` job (spec MEDULLA-CONTRACT-R053.7) builds, lints and tests
only what the diff can break: every workspace package that owns a changed
file, plus every workspace package that depends on one of those (normal,
dev or build edge, transitively). A change to a workspace-wide input
(``Cargo.lock``, the root manifest's ``[workspace]``, the toolchain or
``.cargo/``) affects every package.

Input: ``cargo metadata --format-version 1 --no-deps`` JSON (path argument)
and the changed paths (stdin, one per line, repository-relative).

Output (stdout), one cargo invocation per line, with ``{tool}`` standing for
``clippy`` or ``nextest`` which the caller substitutes. An empty output means
no Rust package is affected.

The per-package feature profiles are the ones main CI already uses for those
packages, so a pull request runs the same gate main runs, only narrower:

* the root facade package: ``--features full`` (``gates-facade``);
* the pyo3 crates: every feature except ``python`` (``gates-pyo3``);
* the storage crate: ``--all-features`` with one test thread (``gates-storage``);
* every other package: ``--all-features`` (``gates-crates``).
"""

from __future__ import annotations

import argparse
import functools
import json
import pathlib
import re
import sys

WORKSPACE_WIDE = (
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/",
    "clippy.toml",
    ".config/nextest.toml",
)

PYO3_FEATURES = (
    "medulla-numeric/analytics,medulla-numeric/contract,medulla-numeric/complex,"
    "medulla-numeric/decision,medulla-numeric/motif,medulla-pyengine/security,"
    "medulla-pyengine/ml-pipeline,medulla-pyengine/reasoning,medulla-pyengine/query,"
    "medulla-pyengine/cypher,medulla-pyengine/tsdb,medulla-pyengine/streaming,"
    "medulla-pyengine/wasm-udf,medulla-pyengine/rdf,medulla-pyengine/blob,medulla-pyengine/jobs,"
    "medulla-pyengine/statechart,medulla-pyengine/finance"
)
PYO3_PACKAGES = ("medulla-numeric", "medulla-pyengine")
SERIAL_PACKAGES = ("medulla-storage",)
# Tested only by their own job, as in the main workspace sweep: medulla-tts-piper
# needs a real onnxruntime under --all-features (job ``tts-piper-inference``).
OWN_JOB_PACKAGES = ("medulla-tts-piper",)


def _packages(metadata: dict) -> dict[str, pathlib.PurePosixPath]:
    root = pathlib.PurePosixPath(metadata["workspace_root"])
    members = set(metadata.get("workspace_members") or [])
    out: dict[str, pathlib.PurePosixPath] = {}
    for pkg in metadata["packages"]:
        if members and pkg["id"] not in members:
            continue
        manifest = pathlib.PurePosixPath(pkg["manifest_path"])
        out[pkg["name"]] = manifest.parent.relative_to(root)
    return out


def _reverse_edges(metadata: dict, names: set[str]) -> dict[str, set[str]]:
    rdeps: dict[str, set[str]] = {name: set() for name in names}
    for pkg in metadata["packages"]:
        if pkg["name"] not in names:
            continue
        for dep in pkg.get("dependencies", []):
            target = dep.get("name")
            if dep.get("path") and target in names and target != pkg["name"]:
                rdeps[target].add(pkg["name"])
    return rdeps


def owner(path: str, packages: dict[str, pathlib.PurePosixPath]) -> str | None:
    """Return the package whose directory is the longest prefix of *path*."""
    best: tuple[int, str] | None = None
    parts = pathlib.PurePosixPath(path).parts
    for name, directory in packages.items():
        dparts = directory.parts if str(directory) != "." else ()
        if parts[: len(dparts)] == dparts and (best is None or len(dparts) > best[0]):
            best = (len(dparts), name)
    return best[1] if best else None


@functools.cache
def _sources(directory: str) -> str:
    """The concatenated Rust sources under *directory* (repository-relative)."""
    if directory == ".":
        # The root package compiles only these; the member crates are scanned
        # as their own packages.
        roots = [pathlib.Path(d) for d in ("src", "tests", "benches", "examples")]
    else:
        roots = [pathlib.Path(directory)]
    files = sorted(f for r in roots if r.is_dir() for f in r.rglob("*.rs"))
    if directory == "." and pathlib.Path("build.rs").is_file():
        files.append(pathlib.Path("build.rs"))
    return "\n".join(f.read_text(encoding="utf-8", errors="replace") for f in files)


def readers(path: str, packages: dict[str, pathlib.PurePosixPath]) -> set[str]:
    """Packages whose Rust sources name *path* or a directory that holds it.

    A test reads a non-crate file through ``include_str!``, ``read_to_string``
    or a path join, so its source holds the literal ``specs/<dir>`` (or the
    full path). Single-segment directories are too broad to be a literal.
    """
    parts = pathlib.PurePosixPath(path).parts
    literals = {"/".join(parts[:n]) for n in range(2, len(parts) + 1)}
    pattern = re.compile(
        "(?:" + "|".join(re.escape(x) for x in sorted(literals)) + r")(?![\w.-])"
    )
    return {
        name
        for name, directory in packages.items()
        if pattern.search(_sources(str(directory)))
    }


def affected(metadata: dict, changed: list[str]) -> list[str]:
    """Owning packages of the changed files plus their reverse dependencies."""
    packages = _packages(metadata)
    if any(
        p == w or (w.endswith("/") and p.startswith(w))
        for p in changed
        for w in WORKSPACE_WIDE
    ):
        return sorted(packages)
    root_pkg = owner("Cargo.toml", packages)
    seeds: set[str] = set()
    for path in changed:
        if path == "Cargo.toml":
            # The root manifest holds [workspace] and shared dependency
            # versions: every member reads it.
            return sorted(packages)
        name = owner(path, packages)
        if name is None or (name == root_pkg and not _is_root_package_source(path)):
            seeds |= readers(path, packages)
            continue
        seeds.add(name)
    rdeps = _reverse_edges(metadata, set(packages))
    out, stack = set(seeds), list(seeds)
    while stack:
        for parent in rdeps.get(stack.pop(), ()):
            if parent not in out:
                out.add(parent)
                stack.append(parent)
    return sorted(out)


def _is_root_package_source(path: str) -> bool:
    """Files that compile into the root package (its src, tests, benches, build)."""
    if path == "build.rs" or path.startswith("src/"):
        return True
    return path.startswith(("tests/", "benches/", "examples/")) and path.endswith(".rs")


def pyo3_features(packages: list[str]) -> str:
    """The pyo3 feature profile, limited to the selected packages' features."""
    return ",".join(
        f for f in PYO3_FEATURES.split(",") if f.split("/", 1)[0] in packages
    )


def commands(metadata: dict, selected: list[str]) -> list[str]:
    packages = _packages(metadata)
    selected = [n for n in selected if n not in OWN_JOB_PACKAGES]
    root_pkg = owner("Cargo.toml", packages)
    default = [
        n
        for n in selected
        if n != root_pkg and n not in PYO3_PACKAGES and n not in SERIAL_PACKAGES
    ]
    pyo3 = [n for n in selected if n in PYO3_PACKAGES]
    serial = [n for n in selected if n in SERIAL_PACKAGES]
    runs: list[str] = []

    def flags(names: list[str]) -> str:
        return " ".join(f"-p {n}" for n in names)

    if default:
        runs.append(f"{{tool}} {flags(default)} --all-features")
    for name in serial:
        runs.append(f"{{tool}} -p {name} --all-features {{serial}}")
    if pyo3:
        runs.append(f"{{tool}} {flags(pyo3)} --features {pyo3_features(pyo3)}")
    if root_pkg in selected:
        runs.append(f"{{tool}} -p {root_pkg} --features full")
    return runs


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("metadata", type=pathlib.Path)
    parser.add_argument(
        "--list", action="store_true", help="print the affected packages, not the runs"
    )
    args = parser.parse_args(argv)
    metadata = json.loads(args.metadata.read_text(encoding="utf-8"))
    changed = [line.strip() for line in sys.stdin if line.strip()]
    selected = affected(metadata, changed)
    lines = selected if args.list else commands(metadata, selected)
    for line in lines:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
