#!/usr/bin/env python3
"""Check the Cargo build profiles keep iteration fast (MEDULLA-CONTRACT-R056.14).

``profiles_match_the_standard`` reads the root ``Cargo.toml`` profiles and the
CI workflow and returns one finding per deviation (empty means pass):

* ``[profile.dev]``: ``codegen-units = 256``, ``incremental = true`` and
  ``debug = "line-tables-only"``.
* ``[profile.test]`` (the train profile): ``codegen-units = 256``,
  ``incremental = true`` and debug information no richer than line tables.
* The shared compile cache stays on: CI builds through ``sccache``.
* The facade ``--timings`` report is the uploaded artifact of CI job
  ``gates-facade``.

Usage: check_build_profiles.py [--root PATH]. Exit status 0 when it passes.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
CODEGEN_UNITS = 256
LINE_TABLES = "line-tables-only"
# Debug levels no richer than line tables.
LEAN_DEBUG = (0, False, "none", "line-directives-only", LINE_TABLES)
CI_WORKFLOW = Path(".github/workflows/ci.yml")


def _check_profile(
    profiles: dict, name: str, debug_ok: tuple, findings: list[str]
) -> None:
    profile = profiles.get(name)
    if profile is None:
        findings.append(f"Cargo.toml: [profile.{name}] is missing")
        return
    if profile.get("codegen-units") != CODEGEN_UNITS:
        findings.append(f"[profile.{name}]: codegen-units must be {CODEGEN_UNITS}")
    if profile.get("incremental") is not True:
        findings.append(f"[profile.{name}]: incremental must be true")
    if profile.get("debug") not in debug_ok:
        findings.append(
            f"[profile.{name}]: debug {profile.get('debug')!r} "
            "is richer than line tables"
        )


def _job_block(workflow: str, job: str) -> str:
    lines = workflow.splitlines()
    start = next(
        (i for i, line in enumerate(lines) if line.rstrip() == f"  {job}:"), None
    )
    if start is None:
        return ""
    block = [lines[start]]
    for line in lines[start + 1 :]:
        if line.startswith("  ") and not line.startswith("   ") and line.strip():
            break
        block.append(line)
    return "\n".join(block)


def profiles_match_the_standard(root: Path = ROOT) -> list[str]:
    """R056.14: the dev and train profiles, compile cache and facade timings."""
    # spec: MEDULLA-CONTRACT-R056.14
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    profiles = manifest.get("profile", {})
    findings: list[str] = []
    _check_profile(profiles, "dev", (LINE_TABLES,), findings)
    _check_profile(profiles, "test", LEAN_DEBUG, findings)
    workflow_path = root / CI_WORKFLOW
    workflow = (
        workflow_path.read_text(encoding="utf-8") if workflow_path.is_file() else ""
    )
    if "RUSTC_WRAPPER: sccache" not in workflow:
        findings.append(f"{CI_WORKFLOW}: the shared compile cache (sccache) is off")
    facade = _job_block(workflow, "gates-facade")
    if "--timings" not in facade or "upload-artifact" not in facade:
        findings.append(
            f"{CI_WORKFLOW}: job gates-facade must build with --timings "
            "and upload the report"
        )
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = profiles_match_the_standard(args.root)
    for finding in findings:
        print(f"FAIL {finding}")
    if findings:
        return 1
    print("build profiles match the standard")
    return 0


if __name__ == "__main__":
    sys.exit(main())
