"""Every workspace crate stays within the line budget after its split (MEDULLA-CONTRACT-R067)."""

from __future__ import annotations

from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
BUDGET_LINES = 25_000
pytestmark = pytest.mark.no_engine


def _rust_lines(crate: str) -> int:
    """Physical lines of Rust under the crate's `src/` (`wc -l` semantics)."""
    src = REPO / "crates" / crate / "src"
    assert src.is_dir(), f"{crate} has no src directory"
    return sum(p.read_bytes().count(b"\n") for p in src.rglob("*.rs"))


def _assert_split(crates: list[str], budget: int = BUDGET_LINES) -> None:
    for crate in crates:
        lines = _rust_lines(crate)
        assert 0 < lines <= budget, f"{crate} holds {lines} lines (budget {budget})"
        manifest = (REPO / "crates" / crate / "Cargo.toml").read_text()
        assert f'name = "{crate}"' in manifest
    workspace = (REPO / "Cargo.toml").read_text()
    layers = (REPO / "pyproject.toml").read_text()
    for crate in crates:
        assert f'"crates/{crate}"' in workspace, f"{crate} is not a workspace member"
        assert f'"{crate}",' in layers, f"{crate} has no layer entry"


# spec: MEDULLA-CONTRACT-R067.3
@pytest.mark.spec("MEDULLA-CONTRACT-R067.3")
def test_catalog_types_split_is_within_budget() -> None:
    _assert_split(["medulla-catalog-source-types", "medulla-catalog-types"])
