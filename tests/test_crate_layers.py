"""Crate dependencies follow declared stability layers (MEDULLA-SDLC-R021)."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-SDLC-R021")]
ROOT = Path(__file__).resolve().parents[1]
_SPEC = importlib.util.spec_from_file_location(
    "check_crate_graph", ROOT / "scripts" / "check_crate_graph.py"
)
assert _SPEC is not None and _SPEC.loader is not None
check_crate_graph = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(check_crate_graph)


def _crate(root: Path, name: str, deps: str = "", dev: str = "") -> None:
    path = root / "crates" / name
    (path / "src").mkdir(parents=True)
    (path / "Cargo.toml").write_text(
        f'[package]\nname = "{name}"\nversion = "0.1.0"\n\n'
        f"[dependencies]\n{deps}\n[dev-dependencies]\n{dev}\n",
        encoding="utf-8",
    )


def _workspace(
    root: Path, layers: dict[str, list[str]], extra: tuple[str, ...] = ()
) -> None:
    members = sorted({c for crates in layers.values() for c in crates} | set(extra))
    (root / "Cargo.toml").write_text(
        "[workspace]\nmembers = ["
        + ", ".join(f'"crates/{m}"' for m in members)
        + "]\n",
        encoding="utf-8",
    )
    lines: list[str] = []
    for name, crates in layers.items():
        units = ", ".join(f'"{c}"' for c in crates)
        lines += [
            "[[tool.pipelines_hooks.coupling.cargo_layers]]",
            f'name = "{name}"',
            f"units = [{units}]",
        ]
    (root / "pyproject.toml").write_text("\n".join(lines) + "\n", encoding="utf-8")


def _findings(root: Path, monkeypatch: pytest.MonkeyPatch) -> list[str]:
    # The fixture workspaces are not buildable; read the manifests directly.
    monkeypatch.setattr(check_crate_graph.shutil, "which", lambda _name: None)
    return check_crate_graph.layer_findings(root)


def test_crate_dependencies_follow_layers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The real tree holds its layers; each fixture rule is refused by location."""
    assert check_crate_graph.layer_findings(ROOT) == []

    down = tmp_path / "down"
    down.mkdir()
    _workspace(down, {"contract": ["a-types"], "engine": ["b-engine"]})
    _crate(down, "a-types", dev='b-engine = { path = "../b-engine" }')
    _crate(down, "b-engine", deps='a-types = { path = "../a-types" }')
    assert _findings(down, monkeypatch) == []

    up = tmp_path / "up"
    up.mkdir()
    _workspace(up, {"contract": ["a-types"], "engine": ["b-engine"]})
    _crate(
        up, "a-types", deps='renamed = { package = "b-engine", path = "../b-engine" }'
    )
    _crate(up, "b-engine")
    assert _findings(up, monkeypatch) == [
        "a-types (contract) depends on b-engine (engine), a later layer"
    ]

    unlayered = tmp_path / "unlayered"
    unlayered.mkdir()
    _workspace(unlayered, {"contract": ["a-types"]}, extra=("c-loose",))
    _crate(unlayered, "a-types")
    _crate(unlayered, "c-loose")
    assert _findings(unlayered, monkeypatch) == [
        "pyproject.toml: c-loose sits in no layer"
    ]

    twice = tmp_path / "twice"
    twice.mkdir()
    _workspace(twice, {"contract": ["a-types"], "engine": ["a-types", "ghost"]})
    _crate(twice, "a-types")
    (twice / "crates" / "ghost").mkdir()
    assert _findings(twice, monkeypatch) == [
        "pyproject.toml: a-types sits in more than one layer",
        "pyproject.toml: ghost is not a workspace crate",
    ]
