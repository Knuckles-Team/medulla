"""Dependency hygiene: no workspace manifest names an unmaintained date-time crate
(MEDULLA-CONTRACT-R064)."""

from __future__ import annotations

import datetime
import re
from pathlib import Path

import pytest
import tomllib

REPO = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine

FORBIDDEN = ("chrono", "chrono-tz", "chrono-tz-build", "humantime")
IGNORED_ADVISORIES = (
    "RUSTSEC-2026-0543",
    "RUSTSEC-2026-0555",
    "RUSTSEC-2026-0559",
    "RUSTSEC-2026-0542",
)
_KINDS = ("dependencies", "dev-dependencies", "build-dependencies")


def _workspace_manifests() -> list[Path]:
    manifests = [REPO / "Cargo.toml"]
    for group in ("crates", "tools"):
        manifests.extend(sorted((REPO / group).glob("*/Cargo.toml")))
    return manifests


def _tables(manifest: dict) -> list[tuple[str, dict]]:
    found = [(kind, manifest[kind]) for kind in _KINDS if kind in manifest]
    for platform, body in manifest.get("target", {}).items():
        found.extend((f"target.{platform}.{kind}", body[kind]) for kind in _KINDS if kind in body)
    workspace = manifest.get("workspace", {})
    if "dependencies" in workspace:
        found.append(("workspace.dependencies", workspace["dependencies"]))
    return found


def _package_name(spec: object, key: str) -> str:
    """The crate a dependency entry resolves to: a renamed entry names it in `package`."""
    if isinstance(spec, dict) and isinstance(spec.get("package"), str):
        return spec["package"]
    return key


@pytest.mark.spec("MEDULLA-CONTRACT-R064")
def test_no_workspace_manifest_names_a_date_time_crate() -> None:
    # spec: MEDULLA-CONTRACT-R064
    failures: list[str] = []
    for path in _workspace_manifests():
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        for table, deps in _tables(manifest):
            for key, spec in deps.items():
                if _package_name(spec, key) in FORBIDDEN:
                    failures.append(f"{path.relative_to(REPO)}: [{table}] names {key}")
    assert not failures, "\n".join(failures)

    deny = tomllib.loads((REPO / ".config" / "deny.toml").read_text(encoding="utf-8"))
    reasons = {entry["id"]: entry["reason"] for entry in deny["advisories"]["ignore"]}
    ledger = (REPO / ".config" / "cargo-audit-allow.txt").read_text(encoding="utf-8")
    today = datetime.date.today()
    limit = today + datetime.timedelta(days=90)
    for advisory in IGNORED_ADVISORIES:
        assert "MEDULLA-CONTRACT-R064" in reasons.get(advisory, ""), advisory
        match = re.search(rf"^{advisory} expires=(\d{{4}}-\d{{2}}-\d{{2}})\b", ledger, re.M)
        assert match, f"{advisory}: no ledger line"
        expiry = datetime.date.fromisoformat(match.group(1))
        assert expiry <= limit, f"{advisory}: expiry {expiry} is more than 90 days ahead"
