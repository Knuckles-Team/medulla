"""Exact-artifact serialized performance certification campaign (G-37).

Run with ``pytest -m release_campaign -k performance``;
see ``campaign_runner.py`` for the opt-in environment.  Additionally
``MEDULLA_PERFORMANCE_AUTHORITY_CONFIG`` names the external authority configuration
(see ``docs/performance_certification.md``).  The JSON and Markdown evidence land in
``MEDULLA_CAMPAIGN_EVIDENCE_DIR`` (default: the test's temporary directory) and
the JSON report is what the multimodal campaign binds.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from campaign_runner import (
    HERE,
    evidence_dir,
    require_opt_in,
    required_env,
    run_harness,
)

pytestmark = [pytest.mark.exact_artifact, pytest.mark.release_campaign]

HARNESS = HERE / "campaign_performance.py"


@pytest.mark.timeout(3600)
def test_exact_binary_performance(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env()
    authority = os.environ.get("MEDULLA_PERFORMANCE_AUTHORITY_CONFIG", "").strip()
    assert authority, (
        "MEDULLA_PERFORMANCE_AUTHORITY_CONFIG is required for performance certification"
    )
    out = evidence_dir(tmp_path)
    json_output = out / "exact-performance.json"
    completed = run_harness(
        HARNESS,
        [
            "--engine-binary",
            env.binary,
            "--engine-sha256",
            env.digest,
            "--authority-config",
            authority,
            "--work-root",
            str(tmp_path / "work"),
            "--json-output",
            str(json_output),
            "--markdown-output",
            str(out / "exact-performance.md"),
        ],
        timeout=3500,
    )
    assert completed.returncode == 0, "performance exact certification failed"
    report = json.loads(json_output.read_text(encoding="utf-8"))
    assert report["status"] == "pass"
    assert report["exact_artifact"]["sha256"] == env.digest
