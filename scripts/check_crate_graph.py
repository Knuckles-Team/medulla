#!/usr/bin/env python3
"""Check the crate graph against the split standard and the stability layers.

Three checks, each a function returning one finding per violation (empty means
pass):

* ``no_contract_modules`` (R056.1): no server or persistence crate holds a
  ``contract_wave`` module, and the ``PersistenceBackend`` trait stays in
  ``crates/medulla-server-api/src/api/persistence.rs``.
* ``features_map_one_to_one`` (R056.16): every handler crate
  (``medulla-h-*``) and wire crate (``medulla-wire*``) is an optional
  dependency of ``medulla-server``, and its ``full`` feature enables each one.
* ``layer_findings`` (MEDULLA-SDLC-R021): every workspace crate sits in exactly
  one ``cargo_layers`` entry of the coupling policy in ``pyproject.toml`` and
  depends only on crates of its own or an earlier layer.

The crate set comes from ``cargo metadata`` when cargo is on the path, else
from the workspace members in the root ``Cargo.toml``.

Usage: check_crate_graph.py [--root PATH]. Exit status 0 when all pass.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

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
LAYERS_FILE = Path("pyproject.toml")
LAYERED_TABLES = ("dependencies", "build-dependencies")


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
    root_package = manifest.get("package", {}).get("name")
    if root_package:
        crates[root_package] = root
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


def _layered_dependencies(manifest: dict[str, Any]) -> set[str]:
    """Names of the crates a manifest depends on outside its dev-dependencies."""
    tables: list[dict[str, Any]] = [manifest.get(table, {}) for table in LAYERED_TABLES]
    for target in manifest.get("target", {}).values():
        tables += [target.get(table, {}) for table in LAYERED_TABLES]
    names: set[str] = set()
    for table in tables:
        for key, spec in table.items():
            renamed = spec.get("package") if isinstance(spec, dict) else None
            names.add(renamed or key)
    return names


def layer_findings(root: Path = ROOT) -> list[str]:
    """R021: each crate sits in one layer and depends on its own or an earlier one."""
    # spec: MEDULLA-SDLC-R021
    layers_path = root / LAYERS_FILE
    if not layers_path.is_file():
        return [f"{LAYERS_FILE}: the crate layers file is missing"]
    policy = tomllib.loads(layers_path.read_text(encoding="utf-8"))
    layers = [
        {"name": entry["name"], "crates": entry["units"]}
        for entry in policy["tool"]["pipelines_hooks"]["coupling"]["cargo_layers"]
    ]
    crates = workspace_crates(root)
    findings: list[str] = []
    rank: dict[str, int] = {}
    for index, layer in enumerate(layers):
        for crate in layer["crates"]:
            if crate in rank:
                findings.append(f"{LAYERS_FILE}: {crate} sits in more than one layer")
            elif crate not in crates:
                findings.append(f"{LAYERS_FILE}: {crate} is not a workspace crate")
            rank.setdefault(crate, index)
    for name, path in sorted(crates.items()):
        if name not in rank:
            findings.append(f"{LAYERS_FILE}: {name} sits in no layer")
            continue
        manifest = tomllib.loads((path / "Cargo.toml").read_text(encoding="utf-8"))
        for dependency in sorted(_layered_dependencies(manifest) & rank.keys()):
            if rank[dependency] > rank[name]:
                findings.append(
                    f"{name} ({layers[rank[name]]['name']}) depends on {dependency} "
                    f"({layers[rank[dependency]]['name']}), a later layer"
                )
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = (
        no_contract_modules(args.root)
        + features_map_one_to_one(args.root)
        + layer_findings(args.root)
    )
    for finding in findings:
        print(f"FAIL {finding}")
    if findings:
        return 1
    print("crate graph: no contract modules; features map one to one; layers hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
