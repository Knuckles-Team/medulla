#!/usr/bin/env python3
"""Check the server crate graph against the split standard (MEDULLA-CONTRACT-R056).

Two checks, each a function returning one finding per violation (empty means
pass):

* ``no_contract_modules`` (R056.1): no server or persistence crate holds a
  ``contract_wave`` module, and the ``PersistenceBackend`` trait stays in
  ``crates/medulla-server-api/src/api/persistence.rs``.
* ``features_map_one_to_one`` (R056.16): every handler crate
  (``medulla-h-*``) and wire crate (``medulla-wire*``) is an optional
  dependency of ``medulla-server``, and its ``full`` feature enables each one.

The crate set comes from ``cargo metadata`` when cargo is on the path, else
from the workspace members in the root ``Cargo.toml``.

Usage: check_crate_graph.py [--root PATH]. Exit status 0 when both pass.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
SERVER_CRATE_PREFIXES = ("medulla-server", "medulla-persistence")
FORBIDDEN_MODULE = "contract_wave"
PERSISTENCE_TRAIT_FILE = Path("crates/medulla-server-api/src/api/persistence.rs")
PERSISTENCE_TRAIT = re.compile(r"^pub trait PersistenceBackend\b", re.MULTILINE)
MOD_DECL = re.compile(
    rf"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+{FORBIDDEN_MODULE}\b", re.MULTILINE
)
SPLIT_CRATE = re.compile(r"^medulla-(?:h-[a-z0-9-]+|wire(?:-[a-z0-9-]+)?)$")


def workspace_crates(root: Path) -> dict[str, Path]:
    """Map each workspace crate name to its directory."""
    if shutil.which("cargo"):
        try:
            out = subprocess.run(
                ["cargo", "metadata", "--format-version", "1", "--no-deps"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            return {
                pkg["name"]: Path(pkg["manifest_path"]).parent
                for pkg in json.loads(out)["packages"]
            }
        except (subprocess.CalledProcessError, OSError, ValueError, KeyError):
            pass
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    crates: dict[str, Path] = {}
    for member in manifest.get("workspace", {}).get("members", []):
        path = root / member
        cargo = path / "Cargo.toml"
        if cargo.is_file():
            name = tomllib.loads(cargo.read_text(encoding="utf-8"))["package"]["name"]
            crates[name] = path
    return crates


def no_contract_modules(root: Path = ROOT) -> list[str]:
    """R056.1: the server contract wave is gone; the persistence contract stays."""
    # spec: MEDULLA-CONTRACT-R056.1
    findings: list[str] = []
    for name, path in sorted(workspace_crates(root).items()):
        if not name.startswith(SERVER_CRATE_PREFIXES):
            continue
        src = path / "src"
        if not src.is_dir():
            continue
        for file in sorted(src.rglob("*")):
            rel = file.relative_to(root) if file.is_relative_to(root) else file
            if file.stem == FORBIDDEN_MODULE:
                findings.append(f"{rel}: {FORBIDDEN_MODULE} module in {name}")
            elif file.suffix == ".rs" and MOD_DECL.search(
                file.read_text(encoding="utf-8", errors="replace")
            ):
                findings.append(f"{rel}: declares mod {FORBIDDEN_MODULE}")
    trait_file = root / PERSISTENCE_TRAIT_FILE
    if not trait_file.is_file() or not PERSISTENCE_TRAIT.search(
        trait_file.read_text(encoding="utf-8")
    ):
        findings.append(
            f"{PERSISTENCE_TRAIT_FILE}: PersistenceBackend trait must stay here"
        )
    return findings


def _enables(entry: str, crate: str) -> bool:
    return entry in (f"dep:{crate}", crate) or entry.startswith(
        (f"{crate}/", f"{crate}?/")
    )


def _feature_closure(features: dict[str, list[str]], start: str) -> set[str]:
    seen: set[str] = set()
    stack = [start]
    entries: set[str] = set()
    while stack:
        feature = stack.pop()
        if feature in seen:
            continue
        seen.add(feature)
        for entry in features.get(feature, []):
            entries.add(entry)
            if entry in features:
                stack.append(entry)
    return entries


def features_map_one_to_one(root: Path = ROOT) -> list[str]:
    """R056.16: each handler and wire crate is optional; `full` enables all."""
    # Verifies MEDULLA-CONTRACT-R056.16: Cargo features map to crates one to one
    # spec: MEDULLA-CONTRACT-R056.16
    manifest_path = root / "crates/medulla-server/Cargo.toml"
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    deps: dict[str, object] = manifest.get("dependencies", {})
    features: dict[str, list[str]] = manifest.get("features", {})
    full = _feature_closure(features, "full")
    findings: list[str] = []
    for crate in sorted(name for name in deps if SPLIT_CRATE.match(name)):
        spec = deps[crate]
        if not (isinstance(spec, dict) and spec.get("optional") is True):
            findings.append(f"medulla-server: {crate} must be an optional dependency")
        if not any(_enables(entry, crate) for entry in full):
            findings.append(f"medulla-server: feature `full` does not enable {crate}")
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = no_contract_modules(args.root) + features_map_one_to_one(args.root)
    for finding in findings:
        print(f"FAIL {finding}")
    if findings:
        return 1
    print("crate graph: no contract modules; features map one to one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
