"""Server-split crate backbone, downward-only layering (MEDULLA-CONTRACT-R056)."""

from __future__ import annotations

import re
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


# The four handler crates whose seam traits (`DecideApi`, `QueryApi`, `AdminApi`,
# `TxnApi`) replace direct cross-handler calls (MEDULLA-CONTRACT-R056.8).
SEAM_HANDLERS = (
    "medulla-h-decide",
    "medulla-h-query",
    "medulla-h-admin",
    "medulla-h-txn",
)
# Crates above the kernel that the store (R056.2) and the kernel (R056.3) must not name.
UPPER_LAYER = tuple(
    crate for crate in BACKBONE if crate not in ("medulla-kernel", "medulla-store")
)


def _declares_module(crate: str, module: str) -> bool:
    pattern = re.compile(rf"^\s*(pub(\([a-z:]+\))?\s+)?mod\s+{module}\b", re.MULTILINE)
    return any(
        pattern.search(path.read_text())
        for path in (REPO / "crates" / crate / "src").rglob("*.rs")
    )


@pytest.mark.spec("MEDULLA-CONTRACT-R056")
@pytest.mark.spec("MEDULLA-CONTRACT-R056.2")
@pytest.mark.spec("MEDULLA-CONTRACT-R056.3")
@pytest.mark.spec("MEDULLA-CONTRACT-R056.8")
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
    # R056.2: the store depends on the kernel and leaf crates only.
    store_deps = _path_deps("medulla-store")
    assert "medulla-kernel" in store_deps
    assert not store_deps & set(UPPER_LAYER), store_deps & set(UPPER_LAYER)
    assert not {d for d in store_deps if d.startswith(("medulla-h-", "medulla-server"))}
    # R056.3: only the kernel declares `persist` and `store_authority`.
    for module in ("persist", "store_authority"):
        assert _declares_module("medulla-kernel", module), module
    assert not _declares_module("medulla-store", "store_authority")
    assert not _declares_module("medulla-server-api", "store_authority")
    assert not _declares_module("medulla-persistence", "store_authority")
    # R056.8: the four seam handler crates hold no edge to each other.
    for crate in SEAM_HANDLERS:
        assert crate in members, crate
        edges = _path_deps(crate) & set(SEAM_HANDLERS)
        assert not edges, f"{crate} depends on {sorted(edges)}"


# The wire protocols of `medulla-wire`: (Cargo feature, `src/lib.rs` module).
WIRE_PROTOCOLS = (
    ("pgwire", "pgwire"),
    ("mysql-wire", "mysql_wire"),
    ("mssql-wire", "mssql_wire"),
    ("sqlite-wire", "sqlite_wire"),
    ("bolt-wire", "bolt_wire"),
)


@pytest.mark.spec("MEDULLA-CONTRACT-R056.9")
def test_wire_crate_gates_one_feature_per_protocol() -> None:
    manifest = tomllib.loads(
        (REPO / "crates" / "medulla-wire" / "Cargo.toml").read_text()
    )
    features = manifest.get("features", {})
    lib = (REPO / "crates" / "medulla-wire" / "src" / "lib.rs").read_text().splitlines()
    for feature, module in WIRE_PROTOCOLS:
        assert feature in features, f"medulla-wire has no feature {feature}"
        declared = [
            i for i, line in enumerate(lib) if line.strip() == f"pub mod {module};"
        ]
        assert len(declared) == 1, (
            f"medulla-wire declares {module} {len(declared)} times"
        )
        gate = f'#[cfg(feature = "{feature}")]'
        preceding = [
            line.strip() for line in lib[max(0, declared[0] - 3) : declared[0]]
        ]
        assert gate in preceding, f"{module} is not gated behind feature {feature}"
    # The wire crate is a sibling of the handler crates: no handler crate depends on it.
    handlers = [
        p.name for p in (REPO / "crates").iterdir() if p.name.startswith("medulla-h-")
    ]
    dependents = [crate for crate in handlers if "medulla-wire" in _path_deps(crate)]
    assert not dependents, f"handler crates depend on medulla-wire: {dependents}"
