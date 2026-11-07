"""Shared runner for the exact-artifact release campaigns.

Each ``test_<campaign>.py`` beside this module runs one ``campaign_<name>.py``
harness as a subprocess against one explicit artifact and checks its evidence.
The opt-in signal is the DEDICATED ``MEDULLA_EXACT_CERTIFICATION=1`` flag,
independent of ``MEDULLA_TEST_BINARY``/``_SHA256`` (those two are also the routine
prebuilt-binary pair set on ordinary runs).  The runner never locates or builds
an executable and campaigns never run concurrently.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, NamedTuple

import pytest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


class ExactCertificationEnv(NamedTuple):
    binary: str
    digest: str
    performance_evidence: str
    performance_digest: str


def _env(name: str) -> str:
    return str(os.environ.get(name, "") or "").strip()


def require_opt_in() -> None:
    if _env("MEDULLA_EXACT_CERTIFICATION") != "1":
        pytest.skip(
            "exact-artifact release certification was not requested "
            "(set MEDULLA_EXACT_CERTIFICATION=1 to opt in)"
        )


def required_env(*, performance: bool = False) -> ExactCertificationEnv:
    """The exact-certification env vars, asserting each required one is set."""
    env = ExactCertificationEnv(
        _env("MEDULLA_TEST_BINARY"),
        _env("MEDULLA_TEST_BINARY_SHA256"),
        _env("MEDULLA_PERFORMANCE_EVIDENCE"),
        _env("MEDULLA_PERFORMANCE_EVIDENCE_SHA256"),
    )
    assert env.binary, "MEDULLA_TEST_BINARY is required for exact certification"
    assert env.digest, "MEDULLA_TEST_BINARY_SHA256 is required for exact certification"
    if performance:
        assert env.performance_evidence, (
            "MEDULLA_PERFORMANCE_EVIDENCE is required for exact certification"
        )
        assert env.performance_digest, (
            "MEDULLA_PERFORMANCE_EVIDENCE_SHA256 is required for exact certification"
        )
    return env


def run_campaign(
    name: str,
    harness: Path,
    env: ExactCertificationEnv,
    tmp_path: Path,
    *,
    extra_args: tuple[str, ...] = (),
    timeout: int = 1100,
) -> dict[str, Any]:
    """Run one harness against the exact artifact; return its evidence."""
    # MEDULLA_CAMPAIGN_EVIDENCE_DIR keeps the evidence (the destination must be new).
    evidence_dir = Path(_env("MEDULLA_CAMPAIGN_EVIDENCE_DIR") or tmp_path)
    evidence_path = evidence_dir / f"exact-{name}.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(harness),
            "--binary",
            env.binary,
            "--binary-sha256",
            env.digest,
            "--output",
            str(evidence_path),
            *extra_args,
        ],
        cwd=ROOT,
        env={**os.environ, "PYTHONPATH": str(HERE)},
        check=False,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    assert completed.returncode == 0, f"{name} exact certification failed"
    evidence: dict[str, Any] = json.loads(evidence_path.read_text(encoding="utf-8"))
    return evidence


def assert_summary(
    evidence: dict[str, Any], env: ExactCertificationEnv, expected: dict[str, object]
) -> None:
    assert evidence["binary"]["sha256"] == env.digest
    for key, value in expected.items():
        assert evidence["summary"][key] == value
