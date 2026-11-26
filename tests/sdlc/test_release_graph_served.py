"""Bundle plans ingest as records (MEDULLA-SDLC-R018.1).

L3: each generated bundle plan under `releases/` (a JSON file) becomes one
ReleaseBundle record with the closure, train order, scenario and open tasks,
and each row of the plan gets an `inBundle` edge. A plan that names an unknown
row refuses `BUNDLE_ROW_UNRESOLVED`. The projection is `project_bundles` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; this test
drives it through the scoped crate test.
"""

from __future__ import annotations

import asyncio
import os
import re
import subprocess
import uuid
from pathlib import Path

import msgpack
import pytest
from conftest import request_context

from medulla.client import MedullaClient

# The projection tests run in the scoped crate test, not the shared engine.

ROOT = Path(__file__).resolve().parents[2]
PROJECTION_RS = (
    ROOT
    / "crates"
    / "medulla-h-ingest"
    / "src"
    / "source_ingestion"
    / "spec_projection.rs"
)
GRAPH_TEST = (
    "source_ingestion::spec_projection::tests::"
    "bundle_plan_becomes_records_with_in_bundle_edges"
)
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


@pytest.mark.no_engine
@pytest.mark.spec("MEDULLA-SDLC-R018.1")
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


@pytest.mark.no_engine
def test_bundle_plan_with_unknown_row_refuses() -> None:
    # spec: MEDULLA-SDLC-R018.1
    _run_engine_test(REFUSAL_TEST)


async def _open_tasks(graph_name: str):
    def node(node_id: str, node_type: str, **props: str) -> dict:
        return {
            "op": "upsert_node",
            "id": node_id,
            "properties": {"type": node_type, **props},
        }

    def edge(source: str, relationship: str, target: str) -> dict:
        return {
            "op": "upsert_edge",
            "source": source,
            "target": target,
            "properties": {"relationship": relationship},
        }

    seed = [
        node(
            "bundle:b",
            "ReleaseBundle",
            train_order="other,demo",
            open_tasks="D-R1,O-R9",
        ),
        node("requirement:demo:D-R1", "Requirement", repo="demo", rowId="D-R1"),
        node("requirement:other:O-R9", "Requirement", repo="other", rowId="O-R9"),
        edge("requirement:demo:D-R1", "inBundle", "bundle:b"),
        edge("requirement:other:O-R9", "inBundle", "bundle:b"),
        node("task:demo:t#T1", "TaskLine", taskId="T1", ticked="false", phase="1"),
        node("task:other:t#T1", "TaskLine", taskId="T1", ticked="false", phase="1"),
        edge("task:demo:t#T1", "forRow", "requirement:demo:D-R1"),
        edge("task:other:t#T1", "forRow", "requirement:other:O-R9"),
    ]
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph_name, "Global")
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(seed, use_bin_type=True)},
            graph_name,
        )
        try:
            return await client.spec_question(
                "open_tasks_of_bundle", graph_name, "bundle:b"
            )
        finally:
            await client.tenants.delete(graph_name)
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R018.2")
def test_open_tasks_of_bundle_in_train_order_match_plan() -> None:
    answer = asyncio.run(_open_tasks(f"sdlc-bundle-{uuid.uuid4().hex[:8]}"))
    if isinstance(answer, (bytes, bytearray)):
        answer = msgpack.unpackb(answer, raw=False)
    # spec: MEDULLA-SDLC-R018.2
    # The generated plan lists the train order `other,demo`; the answer follows it.
    assert [row["repo"] for row in answer["rows"]] == ["other", "demo"]
    assert [row["row"] for row in answer["rows"]] == ["O-R9", "D-R1"]
