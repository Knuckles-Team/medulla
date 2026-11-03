"""MEDULLA-CONTRACT-R056.11: the split scaffold exists as workspace members.

One change created every manifest crate as a workspace member (no lane may
create a crate), so the root `Cargo.toml` is the inventory: every member it
lists must exist on disk as a manifest crate, which is what `cargo metadata`
resolves.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]


# spec: MEDULLA-CONTRACT-R056.11
def test_every_workspace_member_exists_on_disk() -> None:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    members = manifest["workspace"]["members"]
    assert len(members) >= 100, (
        "the split scaffold carries every manifest crate as a workspace "
        f"member; only {len(members)} are listed"
    )

    missing: list[str] = []
    not_crate: list[str] = []
    for member in members:
        path = ROOT / member
        if not path.is_dir():
            missing.append(member)
            continue
        crate = path / "Cargo.toml"
        if not crate.is_file():
            not_crate.append(member)
            continue
        if "package" not in tomllib.loads(crate.read_text(encoding="utf-8")):
            not_crate.append(member)

    assert not missing, f"workspace member(s) absent on disk: {missing}"
    assert not not_crate, (
        f"workspace member(s) without a [package] manifest: {not_crate}"
    )
