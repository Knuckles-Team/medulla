"""Every workspace crate stays within the line budget after its split.

The budget and the split rows are MEDULLA-CONTRACT-R067.
"""

from __future__ import annotations

import tomllib
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


# spec: MEDULLA-CONTRACT-R067.8
@pytest.mark.spec("MEDULLA-CONTRACT-R067.8")
def test_h_admin_split_is_within_budget() -> None:
    _assert_split(["medulla-h-admin-schema", "medulla-h-admin"])


# spec: MEDULLA-CONTRACT-R067.6
@pytest.mark.spec("MEDULLA-CONTRACT-R067.6")
def test_compute_split_is_within_budget() -> None:
    _assert_split(["medulla-compute-assemble", "medulla-compute"])


# spec: MEDULLA-CONTRACT-R067.4
@pytest.mark.spec("MEDULLA-CONTRACT-R067.4")
def test_identity_types_split_is_within_budget() -> None:
    _assert_split(["medulla-identity-core-types", "medulla-identity-types"])


# spec: MEDULLA-CONTRACT-R067.13
@pytest.mark.spec("MEDULLA-CONTRACT-R067.13")
def test_served_tests_split_is_within_budget() -> None:
    harness = "medulla-served-test-harness"
    handler = "medulla-server-served-handler-tests"
    kept = "medulla-server-served-tests"
    _assert_split([harness, handler, kept])
    crates = REPO / "crates"
    # Every handler-crate served test moved; none stays in the kept crate.
    assert not list((crates / kept / "src" / "split_tests").glob("medulla_h_*"))
    assert list((crates / handler / "src" / "split_tests").glob("medulla_h_*.rs"))
    # The harness is a plain library; the two test crates are test-only.
    assert "#![cfg(test)]" not in (crates / harness / "src" / "lib.rs").read_text()
    for crate in (handler, kept):
        assert "#![cfg(test)]" in (crates / crate / "src" / "lib.rs").read_text()
        manifest = (crates / crate / "Cargo.toml").read_text()
        assert f'{harness} = {{ path = "../{harness}" }}' in manifest
    # No crate of the three names another test crate that sits above it.
    for crate in (harness, handler):
        assert f'"../{kept}"' not in (crates / crate / "Cargo.toml").read_text()
    assert f'"../{handler}"' not in (crates / kept / "Cargo.toml").read_text()
    # The old harness paths are gone from the kept crate.
    for module in ("connector_pack_harness", "semantic_live_generation"):
        assert not (crates / kept / "src" / "split_tests" / f"{module}.rs").exists()
        assert (crates / harness / "src" / f"{module}.rs").exists()


# spec: MEDULLA-CONTRACT-R067.10
@pytest.mark.spec("MEDULLA-CONTRACT-R067.10")
def test_h_graph_split_is_within_budget() -> None:
    _assert_split(["medulla-h-graph-gateway", "medulla-h-graph"])


# spec: MEDULLA-CONTRACT-R067.2
@pytest.mark.spec("MEDULLA-CONTRACT-R067.2")
def test_decision_types_split_is_within_budget() -> None:
    _assert_split(
        [
            "medulla-decision-statistical-types",
            "medulla-decision-policy-types",
            "medulla-decision-program-types",
            "medulla-decision-types",
        ]
    )


# spec: MEDULLA-CONTRACT-R067.7
@pytest.mark.spec("MEDULLA-CONTRACT-R067.7")
def test_h_decide_split_is_within_budget() -> None:
    crates = [
        "medulla-h-decide-base",
        "medulla-h-decide-runlog",
        "medulla-h-decide-learning",
        "medulla-h-decide",
        "medulla-h-decide-stat-tests",
        "medulla-h-decide-family-tests",
        "medulla-h-decide-served-tests",
    ]
    _assert_split(crates)
    # No upward edge: a crate never depends on a crate listed after it.
    for rank, crate in enumerate(crates):
        manifest = tomllib.loads((REPO / "crates" / crate / "Cargo.toml").read_text())
        deps = manifest.get("dependencies", {})
        for higher in crates[rank + 1 :]:
            if crate in ("medulla-h-decide-family-tests", "medulla-h-decide-served-tests") and higher == "medulla-h-decide-stat-tests":
                continue
            assert higher not in deps, f"{crate} depends on the higher crate {higher}"
    # The two later test crates reach the shared test helpers of the first one.
    for crate in ("medulla-h-decide-family-tests", "medulla-h-decide-served-tests"):
        manifest = tomllib.loads((REPO / "crates" / crate / "Cargo.toml").read_text())
        assert "test-support" in manifest["dependencies"]["medulla-h-decide-stat-tests"]["features"]
    # The old paths are gone: a moved module is no longer a module of the kept crate.
    kept = REPO / "crates" / "medulla-h-decide" / "src" / "decide"
    for moved in ("stat_log.rs", "stat_replay.rs", "assemble.rs", "family/rank.rs"):
        assert not (kept / moved).exists(), f"{moved} still sits in medulla-h-decide"
    # The family registry and the seam sit above every module they name.
    assert (kept / "family" / "registry.rs").is_file()
    assert (kept / "seam.rs").is_file()
