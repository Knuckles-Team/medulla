"""Exact-artifact reasoning restart, retraction and repair (G-17) release campaign.

Run with ``pytest -m release_campaign -k reasoning_repair``;
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

HARNESS = HERE / "campaign_reasoning_repair.py"


@pytest.mark.timeout(1200)
def test_exact_binary_reasoning_repair(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env()
    evidence = run_campaign("reasoning-repair", HARNESS, env, tmp_path)
    assert_summary(evidence, env, {"cases": 9, "status": "pass"})
