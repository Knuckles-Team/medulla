"""MEDULLA-CONTRACT-R066: the embedded analytical database stays out of dev builds.

The feature `source-duckdb` is outside `default`, `full` and `cluster`; only the
serving and release builds enable it.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
SERVING_SCRIPTS = (
    "scripts/build_release_server.sh",
    "scripts/promote_engine.sh",
    "scripts/wheel_privacy_gate.sh",
)


def _features() -> dict[str, list[str]]:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    return manifest["features"]


def closure(
    features: dict[str, list[str]], roots: list[str]
) -> tuple[set[str], set[str]]:
    """Return the feature names and the `dep:` entries reachable from `roots`."""
    seen: set[str] = set()
    deps: set[str] = set()
    stack = list(roots)
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        seen.add(name)
        for entry in features.get(name, []):
            if entry.startswith("dep:"):
                deps.add(entry)
            elif "/" not in entry:
                stack.append(entry.removesuffix("?"))
    return seen, deps


# spec: MEDULLA-CONTRACT-R066.1
def test_feature_graph_keeps_duckdb_out_of_the_default_build() -> None:
    features = _features()
    for root in ("default", "full", "node", "cluster"):
        if root not in features:
            continue
        seen, deps = closure(features, [root])
        assert "source-duckdb" not in seen, f"{root} reaches source-duckdb"
        assert "dep:duckdb" not in deps, f"{root} reaches dep:duckdb"
    assert "dep:duckdb" in features["source-duckdb"]
    for script in SERVING_SCRIPTS:
        text = (ROOT / script).read_text(encoding="utf-8")
        assert "source-duckdb" in text, f"{script} must enable source-duckdb"
