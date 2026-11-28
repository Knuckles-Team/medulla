"""Every workspace crate stays within the line budget after its split.

The budget and the split rows are MEDULLA-CONTRACT-R067.
"""

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


# spec: MEDULLA-CONTRACT-R067.5
@pytest.mark.spec("MEDULLA-CONTRACT-R067.5")
def test_decision_stats_split_is_within_budget() -> None:
    _assert_split(["medulla-decision-stats-records", "medulla-decision-stats"])


# spec: MEDULLA-CONTRACT-R067.12
@pytest.mark.spec("MEDULLA-CONTRACT-R067.12")
def test_server_router_split_is_within_budget() -> None:
    _assert_split(["medulla-server-router-arms", "medulla-server-router"])
    arms = REPO / "crates" / "medulla-server-router-arms"
    router = REPO / "crates" / "medulla-server-router"
    # No upward edge: the arms crate never names the crate that composes it.
    assert 'medulla-server-router"' not in (arms / "Cargo.toml").read_text()
    for source in arms.rglob("*.rs"):
        assert "medulla_server_router::" not in source.read_text(), source
    assert "medulla-server-router-arms" in (router / "Cargo.toml").read_text()
    # The old paths are gone: a moved arm is no longer a module of the router.
    moved = [
        "service_control",
        "source_ingest",
        "share",
        "lifecycle",
        "sdlc",
        "channels",
    ]
    for name in moved:
        assert not (router / "src" / "dispatch" / "router" / f"{name}.rs").exists(), (
            name
        )
        assert (arms / "src" / "dispatch" / "router" / f"{name}.rs").exists(), name


# spec: MEDULLA-CONTRACT-R067.11
@pytest.mark.spec("MEDULLA-CONTRACT-R067.11")
def test_h_query_split_is_within_budget() -> None:
    _assert_split(["medulla-h-query-read", "medulla-h-query"])
# spec: MEDULLA-CONTRACT-R067.9
@pytest.mark.spec("MEDULLA-CONTRACT-R067.9")
def test_h_ingest_split_is_within_budget() -> None:
    _assert_split(["medulla-h-ingest-repository", "medulla-h-ingest"])
