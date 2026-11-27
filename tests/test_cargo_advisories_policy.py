"""MEDULLA-CONTRACT-R065.6: the dependency advisory gate passes without new ignores.

`zip` and `url` are on releases past their advisories, so `cargo deny check
advisories` exits 0 and `.config/deny.toml` holds no ignore that the merge base did
not hold.
"""

from __future__ import annotations

import datetime
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
DENY = ROOT / ".config" / "deny.toml"


def _ignore_ids(text: str) -> set[str]:
    return {e["id"] for e in tomllib.loads(text)["advisories"].get("ignore", [])}


def _base_ignore_ids() -> set[str] | None:
    try:
        base = subprocess.run(
            ["bash", str(ROOT / "scripts" / "gate_base_ref.sh")],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        shown = subprocess.run(
            ["git", "show", f"{base}:.config/deny.toml"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return _ignore_ids(shown)


# spec: MEDULLA-CONTRACT-R065.6
def test_cargo_deny_advisories_pass_without_new_ignores() -> None:
    base = _base_ignore_ids()
    if base is not None:
        added = _ignore_ids(DENY.read_text(encoding="utf-8")) - base
        assert not added, f"new advisory ignores since the merge base: {sorted(added)}"
    probe = (
        subprocess.run(["cargo", "--version"], capture_output=True, text=True)
        if shutil.which("cargo") and shutil.which("cargo-deny")
        else None
    )
    if probe is None or probe.returncode != 0:
        pytest.skip("cargo-deny is unavailable here; this gate runs on a build host")
    proc = subprocess.run(
        ["cargo", "deny", "--all-features", "check", "advisories"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr[-3000:]


BANNED = ("chrono", "chrono-tz", "chrono-tz-build", "humantime")
LEDGER = ROOT / ".config" / "cargo-audit-allow.txt"
ROW = "MEDULLA-CONTRACT-R064"


def _manifests() -> list[Path]:
    found = [ROOT / "Cargo.toml"]
    for group in ("crates", "tools"):
        found += sorted((ROOT / group).glob("*/Cargo.toml"))
    return found


def _named_in(node: object, path: str = "") -> list[str]:
    hits: list[str] = []
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}" if path else key
            if key in BANNED and path.rsplit(".", 1)[-1] in {
                "dependencies",
                "dev-dependencies",
                "build-dependencies",
            }:
                hits.append(here)
            if isinstance(value, dict) and value.get("package") in BANNED:
                hits.append(f"{here} (package = {value['package']})")
            hits += _named_in(value, here)
    return hits


# spec: MEDULLA-CONTRACT-R064
def test_no_workspace_manifest_names_a_date_time_crate() -> None:
    offenders: list[str] = []
    for manifest in _manifests():
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        offenders += [
            f"{manifest.relative_to(ROOT)}: [{table}]" for table in _named_in(data)
        ]
    assert not offenders, f"manifests name a banned date-time crate: {offenders}"

    ignores = tomllib.loads(DENY.read_text(encoding="utf-8"))["advisories"]["ignore"]
    assert len(ignores) == 4, "the four date-time advisories are the only ignores"
    ledger: dict[str, datetime.date] = {}
    for line in LEDGER.read_text(encoding="utf-8").splitlines():
        found = re.match(r"(RUSTSEC-\d{4}-\d{4}) expires=(\d{4}-\d{2}-\d{2})\b", line)
        if found:
            ledger[found[1]] = datetime.date.fromisoformat(found[2])
    limit = datetime.date.today() + datetime.timedelta(days=90)
    for entry in ignores:
        assert ROW in entry["reason"], f"{entry['id']} reason does not cite {ROW}"
        assert entry["id"] in ledger, f"{entry['id']} has no ledger line with an expiry"
        assert ledger[entry["id"]] <= limit, f"{entry['id']} expiry is over 90 days out"
