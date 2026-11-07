"""Exact-artifact protocol authorization and read isolation campaign (G-04).

Run with ``pytest -m release_campaign -k protocol_authorization``; see
``campaign_runner.py`` for the opt-in environment.
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

HARNESS = HERE / "campaign_protocol_authorization.py"


@pytest.mark.timeout(1200)
def test_exact_binary_protocol_authorization(tmp_path: Path) -> None:
    require_opt_in()
    env = required_env()
    evidence = run_campaign("protocol-authorization", HARNESS, env, tmp_path)
    assert_summary(
        evidence,
        env,
        {"data_path_cases": 14, "protocol_cases": 10, "status": "pass"},
    )
