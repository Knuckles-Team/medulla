"""Exact-artifact multimodal (document, image, audio, video) release campaign (G-14).

Run with ``pytest -m release_campaign -k multimodal``;
see ``campaign_runner.py`` for the opt-in environment.  The campaign also binds a
digest-pinned G-37 performance report for the same engine digest.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from campaign_runner import (
    HERE,
    assert_summary,
    require_opt_in,
    required_env,
    run_campaign,
)

pytestmark = [pytest.mark.exact_artifact, pytest.mark.release_campaign]

HARNESS = HERE / "campaign_multimodal.py"


@pytest.mark.timeout(1200)
def test_exact_binary_multimodal(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env(performance=True)
    evidence = run_campaign(
        "multimodal",
        HARNESS,
        env,
        tmp_path,
        extra_args=(
            "--performance-evidence",
            env.performance_evidence,
            "--performance-evidence-sha256",
            env.performance_digest,
        ),
    )
    assert_summary(
        evidence,
        env,
        {
            "dimensions_per_modality": 12,
            "fault_cases": 16,
            "modalities": 4,
            "passed": 4,
            "status": "pass",
        },
    )
    assert evidence["binary"]["sealed_copy_verified"] is True
    assert evidence["performance"]["report_sha256"] == env.performance_digest
    assert all(row["component_tck_not_applicable"] == 0 for row in evidence["matrix"])
