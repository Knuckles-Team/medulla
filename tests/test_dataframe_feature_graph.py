"""MEDULLA-CONTRACT-R066.4: the dataframe library family is behind its own feature.

The `polars-*` crates are reached only through `lake` and `obs`. The umbrella
feature `dataframe` names them, `default`, `full` and `cluster` reach neither, and
the serving and release builds pass `dataframe`.
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


def _closure(
    features: dict[str, list[str]], roots: list[str]
) -> tuple[set[str], set[str]]:
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


# spec: MEDULLA-CONTRACT-R066.4
def test_feature_graph_keeps_the_dataframe_family_out_of_the_default_build() -> None:
    features = _features()
    for root in ("default", "full", "cluster"):
        seen, deps = _closure(features, [root])
        assert "lake" not in seen and "obs" not in seen, root
        polars = sorted(d for d in deps if d.startswith("dep:polars"))
        assert not polars, f"{root} reaches {polars}"
    seen, deps = _closure(features, ["dataframe"])
    assert {"lake", "obs"} <= seen
    assert any(d.startswith("dep:polars") for d in deps)
    for script in SERVING_SCRIPTS:
        text = (ROOT / script).read_text(encoding="utf-8")
        assert "dataframe" in text, f"{script} must enable dataframe"
