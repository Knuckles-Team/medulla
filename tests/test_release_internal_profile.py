"""MEDULLA-CONTRACT-R068: internal deploys build with the `release-internal` profile.

The profile is thin LTO, 16 codegen units, opt-level 3 and unwinding panics. The
published-wheel profiles keep their recorded values, and no workflow publishes with
the internal profile.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]

RELEASE = {
    "opt-level": 3,
    "lto": "thin",
    "codegen-units": 1,
    "panic": "unwind",
    "strip": True,
}
RELEASE_SERVER = {
    "inherits": "release",
    "opt-level": "s",
    "lto": "fat",
    "codegen-units": 1,
    "panic": "unwind",
    "debug": "line-tables-only",
    "strip": "none",
}


def _profiles() -> dict:
    return tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["profile"]


# spec: MEDULLA-CONTRACT-R068.1
def test_profile_is_thin_lto_16_units_opt3_unwind() -> None:
    profiles = _profiles()
    assert profiles["release-internal"] == {
        "inherits": "release",
        "lto": "thin",
        "codegen-units": 16,
        "opt-level": 3,
        "panic": "unwind",
    }
    for key, value in RELEASE.items():
        assert profiles["release"].get(key) == value, f"release.{key} changed"
    for key, value in RELEASE_SERVER.items():
        assert profiles["release-server"].get(key) == value, (
            f"release-server.{key} changed"
        )


# spec: MEDULLA-CONTRACT-R068.2
def test_promote_engine_builds_with_release_internal() -> None:
    script = (ROOT / "scripts" / "promote_engine.sh").read_text(encoding="utf-8")
    assert "--profile release-internal" in script
    assert "target/release-internal/medulla-server" in script
    assert "--release " not in script.replace("--release-", "")
    for workflow in sorted((ROOT / ".github" / "workflows").glob("*.yml")):
        assert "release-internal" not in workflow.read_text(encoding="utf-8"), (
            workflow.name
        )
    for script_path in sorted((ROOT / "scripts").glob("*.sh")):
        if script_path.name == "promote_engine.sh":
            continue
        text = script_path.read_text(encoding="utf-8")
        if "release-internal" in text:
            assert "maturin" not in text and "twine" not in text, script_path.name
