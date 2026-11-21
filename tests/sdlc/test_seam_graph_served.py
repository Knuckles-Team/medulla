"""Contracts form the seam graph (MEDULLA-SDLC-R014.4).

L4: each hand-written `contracts/<seam>.md` becomes one Contract record that
names the provider and consumer repositories and rows, lists the wire shape and
the refusal codes and links the authoritative types and test vectors. Each row
a contract names gets a `crossesSeam` edge to it, so a requirement resolves to
the owning contract and both repositories. A contract row that no spec holds
refuses `CONTRACT_ROW_UNRESOLVED`. The projection is `project_contracts` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; this test
drives it through the scoped crate test.
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
GRAPH_TEST = "source_ingestion::spec_projection::tests::contract_files_form_the_seam_graph"
REFUSAL_TEST = "source_ingestion::spec_projection::tests::contract_with_unknown_row_refuses"


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
            f"engine contract test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


def test_concept_resolves_to_owning_contract_and_both_repositories() -> None:
    source = PROJECTION_RS.read_text(encoding="utf-8")
    assert "pub fn project_contracts" in source
    assert "CONTRACT_ROW_UNRESOLVED" in source

    out = _run_engine_test(GRAPH_TEST)
    contract = re.search(r"^CONTRACT\t(\S+)\t(\S+)\t(\S+)$", out, flags=re.MULTILINE)
    assert contract is not None
    providers, consumers, codes = contract.groups()
    sources = set(re.findall(r"^SEAM_EDGE\t(\S+)$", out, flags=re.MULTILINE))

    # spec: MEDULLA-SDLC-R014.4
    assert providers == "demo"
    assert consumers == "other"
    assert codes == "DEMO_REFUSED"
    assert sources == {
        "requirement:demo:DEMO-R001",
        "requirement:demo:DEMO-R002",
        "requirement:other:OTHER-R009",
    }


def test_contract_with_unresolved_row_refuses() -> None:
    # spec: MEDULLA-SDLC-R014.4
    _run_engine_test(REFUSAL_TEST)
