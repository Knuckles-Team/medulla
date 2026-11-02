"""Every crate scaffolded for the split exists on disk (MEDULLA-CONTRACT-R056.11)."""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

REPO = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-CONTRACT-R056.11")
def test_every_workspace_member_exists_on_disk() -> None:
    members = tomllib.loads((REPO / "Cargo.toml").read_text())["workspace"]["members"]
    assert members
    missing = [m for m in members if not (REPO / m / "Cargo.toml").is_file()]
    assert missing == []
