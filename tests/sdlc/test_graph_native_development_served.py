"""Spec rows are requirement records with caller and proof edges (MEDULLA-SDLC-R012.2).

L4: the engine projects each requirements row of a spec directory into one
requirement node (ID, owner repository, title, derived state) and derives the
`dependsOn`, `implementedBy` and `provedBy` edges from the row text. The
projection is `project_requirements` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; it writes
through the mutation gateway as one batch. This test drives the engine
projection through the scoped crate test and asserts the nodes and edges it
reports for the fixture rows.
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

ROOT = Path(__file__).resolve().parents[2]
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
    "spec_rows_become_requirement_nodes_with_edges"
)
REFUSAL_TEST = (
    "source_ingestion::spec_projection::tests::"
    "unknown_depends_id_refuses_and_writes_nothing"
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
            f"engine projection test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


@pytest.mark.spec("MEDULLA-SDLC-R012.2")
def test_spec_rows_become_requirement_nodes_with_edges() -> None:
    source = PROJECTION_RS.read_text(encoding="utf-8")
    assert "pub fn project_requirements" in source
    assert "REQUIREMENT_UNRESOLVED" in source

    out = _run_engine_test(PROJECT_TEST)
    nodes = {
        row_id: (repo, title, state)
        for row_id, repo, title, state in re.findall(
            r"^REQ_NODE\t(\S+)\t(\S+)\t([^\t]+)\t(\S+)$", out, flags=re.MULTILINE
        )
    }
    edges = set(re.findall(r"^REQ_EDGE\t(\S+)\t(\S+)\t(\S+)$", out, flags=re.MULTILINE))

    # spec: MEDULLA-SDLC-R012.2
    assert nodes == {
        "DEMO-R001": ("demo", "Base.", "VERIFIED"),
        "DEMO-R002": ("demo", "Second.", "SPECIFIED"),
        "DEMO-R003": ("demo", "Third.", "SPECIFIED"),
    }
    assert ("DEMO-R002", "dependsOn", "requirement:demo:DEMO-R001") in edges
    assert ("DEMO-R003", "dependsOn", "requirement:demo:DEMO-R002") in edges
    assert ("DEMO-R001", "implementedBy", "symbol:serve_base") in edges
    assert (
        "DEMO-R002",
        "provedBy",
        "test:crates/demo/tests/second.rs::runs",
    ) in edges


def test_unresolved_requirement_id_refuses() -> None:
    # spec: MEDULLA-SDLC-R012.2
    _run_engine_test(REFUSAL_TEST)
