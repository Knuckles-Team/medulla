"""Server-split crate backbone, downward-only layering (MEDULLA-CONTRACT-R056)."""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

REPO = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine

# Rollup layer order: a crate may depend only on crates earlier in this list.
BACKBONE = (
    "medulla-kernel",
    "medulla-store",
    "medulla-server-api",
    "medulla-persistence",
    "medulla-raft",
    "medulla-wire",
    "medulla-server",
)


def _members() -> set[str]:
    workspace = tomllib.loads((REPO / "Cargo.toml").read_text())["workspace"]
    return {Path(member).name for member in workspace["members"]}


def _path_deps(crate: str) -> set[str]:
    manifest = tomllib.loads((REPO / "crates" / crate / "Cargo.toml").read_text())
    names: set[str] = set()
    for table in ("dependencies", "build-dependencies"):
        for name, spec in manifest.get(table, {}).items():
            if isinstance(spec, dict) and ("path" in spec or spec.get("workspace")):
                names.add(spec.get("package", name))
    return names


@pytest.mark.spec("MEDULLA-CONTRACT-R056")
def test_backbone_crates_exist_and_depend_only_downward() -> None:
    members = _members()
    missing = [crate for crate in BACKBONE if crate not in members]
    assert not missing, f"split backbone crates missing from the workspace: {missing}"
    upward = [
        f"{crate} -> {dep}"
        for index, crate in enumerate(BACKBONE)
        for dep in _path_deps(crate)
        if dep in BACKBONE[index + 1 :]
    ]
    assert not upward, f"backbone crates depend upward: {upward}"
