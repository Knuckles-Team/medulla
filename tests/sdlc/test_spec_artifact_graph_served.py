"""Incremental ingest by file digest (MEDULLA-SDLC-R014.2).

L3: the engine keeps one digest per spec artifact file and re-ingests only the
files whose digest changed. A file that leaves the tree loses its records. The
job is `reingest_changed_artifacts` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; this test
drives it through the scoped crate test, which also asserts the one-second
budget for a one-file change.
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
REINGEST_TEST = (
    "source_ingestion::spec_projection::tests::one_file_change_reingests_only_that_file"
)


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
            f"engine reingest test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


@pytest.mark.spec("MEDULLA-SDLC-R014.2")
def test_one_file_change_reingests_only_that_file_within_budget() -> None:
    assert "pub fn reingest_changed_artifacts" in PROJECTION_RS.read_text(
        encoding="utf-8"
    )
    out = _run_engine_test(REINGEST_TEST)

    # spec: MEDULLA-SDLC-R014.2
    assert re.search(r"^REINGEST\tmore/requirements.md$", out, flags=re.MULTILINE)
    assert re.search(r"^UNCHANGED\t1$", out, flags=re.MULTILINE)
    assert re.search(r"^REMOVED\tmore/requirements.md$", out, flags=re.MULTILINE)


CITE_TEST = (
    "source_ingestion::spec_projection::tests::"
    "index_batch_cites_requirements_once_per_changeset"
)


@pytest.mark.spec("MEDULLA-SDLC-R014.3")
def test_trailers_become_citation_edges() -> None:
    assert "pub fn cite_requirements" in PROJECTION_RS.read_text(encoding="utf-8")
    out = _run_engine_test(CITE_TEST)

    # spec: MEDULLA-SDLC-R014.3
    assert re.search(
        r"^CITE\tchangeset:demo:c1\trequirement:demo:DEMO-R001$", out, flags=re.MULTILINE
    )
    assert re.search(
        r"^CITE_REFUSED\tREQUIREMENT_UNRESOLVED: DEMO-R999 \(cited by changeset c2\)",
        out,
        flags=re.MULTILINE,
    )
