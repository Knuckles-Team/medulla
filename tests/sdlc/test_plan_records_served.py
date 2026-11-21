"""Plan sections are records with phase and cover edges (MEDULLA-SDLC-R014.7).

L3: a changed `plan.md` of a spec tree becomes one `PlanSection` record per level-2
heading with its span, digest and `phase`, and one `covers` edge to each requirement
row the section names. A `Phase <N>` heading outside the template phases refuses
`PLAN_PHASE_UNKNOWN`. The projection is `project_plans` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`, reached through
`reingest_changed_artifacts`; this test drives it through the scoped crate tests.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

pytestmark = (
    pytest.mark.no_engine
)  # The projection runs in the scoped crate test, not the shared engine.

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
UNIT_TEST = PREFIX + "plan_sections_carry_phase_and_cover_rows_and_unknown_phase_refuses"
REINGEST_TEST = PREFIX + "a_changed_plan_file_reingests_its_sections_with_covers_edges"


def _run_engine_test(name: str) -> None:
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
            f"engine plan test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )


@pytest.mark.spec("MEDULLA-SDLC-R014.7")
def test_plan_sections_become_records_with_phase_and_cover_edges() -> None:
    source = PROJECTION_RS.read_text(encoding="utf-8")
    assert "pub fn project_plans" in source
    assert "PLAN_PHASE_UNKNOWN" in source or "PlanPhaseUnknown" in source

    _run_engine_test(UNIT_TEST)
    _run_engine_test(REINGEST_TEST)
