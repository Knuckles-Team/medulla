"""Exact-artifact KnowledgeBatch (G-15) release campaign.

Run with ``pytest -m release_campaign -k knowledge_batch``;
see ``campaign_runner.py`` for the opt-in environment.
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

HARNESS = HERE / "campaign_knowledge_batch.py"


@pytest.mark.timeout(1200)
def test_exact_binary_knowledge_batch(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env()
    evidence = run_campaign("knowledge-batch", HARNESS, env, tmp_path)
    assert_summary(
        evidence,
        env,
        {"families": 7, "requirements": 7, "snapshot_cases": 7, "status": "pass"},
    )
