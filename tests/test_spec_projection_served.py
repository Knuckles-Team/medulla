"""Governed documentation projection (MEDULLA-SDLC-R009).

L3: each spec document projects into a `SpecDocument` node and one
`SpecSection` node per heading, with the source span and the digest of the
section bytes. The nodes hold spans and digests only; there is no second copy of
the text. The projection is `project` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`, called per
changed file by the scoped repository index; this test drives it through the
scoped crate test.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

pytestmark = (
    pytest.mark.no_engine
)  # The projection runs in the scoped crate test, not the shared engine.

ROOT = Path(__file__).resolve().parents[1]
PROJECTION_RS = (
    ROOT
    / "crates"
    / "medulla-h-ingest"
    / "src"
    / "source_ingestion"
    / "spec_projection.rs"
)
PROJECT_TEST = (
    "source_ingestion::spec_projection::tests::"
    "spec_document_projects_sections_with_spans"
)


def test_spec_document_projects_sections_with_spans() -> None:
    assert "pub fn project(" in PROJECTION_RS.read_text(encoding="utf-8")
    proc = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "medulla-h-ingest",
            "--lib",
            "--",
            "--exact",
            PROJECT_TEST,
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
            f"engine projection test failed:\n{proc.stdout}\n{proc.stderr}"
        )
    sections = re.findall(
        r"^SECTION\t(\S+)\t(\d)\t(\d+)\t(\d+)$", proc.stdout, flags=re.MULTILINE
    )

    # spec: MEDULLA-SDLC-R009
    assert sections == [
        ("Title", "1", "1", "4"),
        ("Rows", "2", "5", "11"),
        ("Tasks", "2", "12", "13"),
    ]
