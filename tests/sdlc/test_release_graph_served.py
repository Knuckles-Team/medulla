"""Bundle plans ingest as records (MEDULLA-SDLC-R018.1).

L3: each generated bundle plan under `releases/` (a JSON file) becomes one
ReleaseBundle record with the closure, train order, scenario and open tasks,
and each row of the plan gets an `inBundle` edge. A plan that names an unknown
row refuses `BUNDLE_ROW_UNRESOLVED`. The projection is `project_bundles` in
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
GRAPH_TEST = "source_ingestion::spec_projection::tests::bundle_plan_becomes_records_with_in_bundle_edges"
REFUSAL_TEST = (
    "source_ingestion::spec_projection::tests::bundle_plan_with_unknown_row_refuses"
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
            f"engine bundle test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


def test_bundle_plan_becomes_records_with_in_bundle_edges() -> None:
    source = PROJECTION_RS.read_text(encoding="utf-8")
    assert "pub fn project_bundles" in source
    assert "BUNDLE_ROW_UNRESOLVED" in source

    out = _run_engine_test(GRAPH_TEST)
    bundle = re.search(r"^BUNDLE\t(\S+)\t(\S+)\t(\S+)$", out, flags=re.MULTILINE)
    assert bundle is not None
    scenario, train_order, closure = bundle.groups()
    members = set(re.findall(r"^IN_BUNDLE\t(\S+)$", out, flags=re.MULTILINE))

    # spec: MEDULLA-SDLC-R018.1
    assert scenario == "DEMO-SCENARIO-R001"
    assert train_order == "other,demo"
    assert closure == "DEMO-R001,DEMO-R002,OTHER-R009"
    assert members == {
        "requirement:demo:DEMO-R001",
        "requirement:demo:DEMO-R002",
        "requirement:other:OTHER-R009",
    }


def test_bundle_plan_with_unknown_row_refuses() -> None:
    # spec: MEDULLA-SDLC-R018.1
    _run_engine_test(REFUSAL_TEST)
