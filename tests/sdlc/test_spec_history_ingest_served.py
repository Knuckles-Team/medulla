"""Spec history ingest (MEDULLA-SDLC-R014.12).

The job is `link_history_to_rows` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; like the other
spec artifact jobs of this directory it is driven through the scoped crate
tests, which also assert the budget of a delta pass.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

pytestmark = (
    pytest.mark.no_engine
)  # The job runs in the scoped crate test, not the shared engine.

ROOT = Path(__file__).resolve().parents[2]
PROJECTION_RS = (
    ROOT
    / "crates"
    / "medulla-h-ingest"
    / "src"
    / "source_ingestion"
    / "spec_projection.rs"
)
PREFIX = "source_ingestion::spec_projection::tests::"


def _run_engine_test(name: str) -> str:
    proc = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "medulla-h-ingest",
            "--lib",
            "--",
            "--exact",
            name,
            "--nocapture",
        ],
        cwd=ROOT,
        env={**os.environ, "CARGO_TERM_COLOR": "never"},
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    if proc.returncode != 0:
        raise AssertionError(
            f"engine history test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


@pytest.mark.spec("MEDULLA-SDLC-R014.12")
def test_row_commit_test_and_depends_on_are_queryable() -> None:
    assert "pub fn link_history_to_rows" in PROJECTION_RS.read_text(encoding="utf-8")
    out = _run_engine_test(
        PREFIX + "history_links_rows_to_commits_tests_and_dependencies"
    )

    # spec: MEDULLA-SDLC-R014.12
    assert re.search(r"^HISTORY\t\d+\t\d+$", out, flags=re.MULTILINE)


@pytest.mark.spec("MEDULLA-SDLC-R014.12")
def test_delta_reads_only_new_commits_within_budget() -> None:
    out = _run_engine_test(PREFIX + "history_delta_emits_only_new_commits")

    # spec: MEDULLA-SDLC-R014.12
    assert re.search(r"^HISTORY_DELTA\t1\t\d+$", out, flags=re.MULTILINE)
