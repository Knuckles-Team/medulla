"""Opt-in serial execution of the exact-artifact release campaigns still run together.

The harnesses own every engine process and receive one explicit artifact.  The test
never locates or builds an executable and never starts campaigns concurrently.
See ``release_campaigns/campaign_runner.py`` for the opt-in signal.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from campaign_runner import (
    assert_summary,
    require_opt_in,
    required_env,
    run_campaign,
)

pytestmark = pytest.mark.exact_artifact

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
CAMPAIGNS = (
    (
        "multimodal",
        SCRIPTS / "certify_exact_multimodal.py",
        {
            "dimensions_per_modality": 12,
            "fault_cases": 16,
            "modalities": 4,
            "passed": 4,
            "status": "pass",
        },
    ),
    (
        "knowledge-batch",
        SCRIPTS / "certify_exact_knowledge_batch.py",
        {"families": 7, "requirements": 7, "snapshot_cases": 7, "status": "pass"},
    ),
    (
        "reasoning-repair",
        SCRIPTS / "certify_exact_reasoning_repair.py",
        {"cases": 9, "status": "pass"},
    ),
)


@pytest.mark.timeout(4800)
def test_exact_release_campaigns_are_serial_and_complete(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env(performance=True)
    for name, harness, expected_summary in CAMPAIGNS:
        extra: tuple[str, ...] = ()
        if name == "multimodal":
            extra = (
                "--performance-evidence",
                env.performance_evidence,
                "--performance-evidence-sha256",
                env.performance_digest,
            )
        evidence = run_campaign(name, harness, env, tmp_path, extra_args=extra)
        assert_summary(evidence, env, expected_summary)
        if name == "multimodal":
            assert evidence["binary"]["sealed_copy_verified"] is True
            assert evidence["performance"]["report_sha256"] == env.performance_digest
            assert all(
                row["component_tck_not_applicable"] == 0 for row in evidence["matrix"]
            )
