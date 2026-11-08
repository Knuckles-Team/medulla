"""The crate layers live in the coupling policy table (MEDULLA-SDLC-R021.1)."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
import tomllib

pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-SDLC-R021.1")]
ROOT = Path(__file__).resolve().parents[1]


def _entries() -> list[dict[str, Any]]:
    policy = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    return policy["tool"]["pipelines_hooks"]["coupling"]["cargo_layers"]


def _members() -> set[str]:
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    names: set[str] = set()
    for member in workspace["workspace"]["members"]:
        manifest = ROOT / member / "Cargo.toml"
        package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
        names.add(package["name"])
    return names


def test_every_workspace_crate_is_in_exactly_one_layer_entry() -> None:
    entries = _entries()
    assert [entry["name"] for entry in entries] == [
        "contract",
        "kernels",
        "storage",
        "graph-core",
        "engine",
        "service",
        "handlers",
        "server",
        "adapters",
    ]
    listed = [unit for entry in entries for unit in entry["units"]]
    assert len(listed) == len(set(listed)), "a crate sits in more than one entry"
    members = _members()
    assert members <= set(listed), sorted(members - set(listed))
    assert not (ROOT / "architecture" / "crate-layers.yml").exists()
