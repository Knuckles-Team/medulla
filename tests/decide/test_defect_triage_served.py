"""MEDULLA-SDLC-R004.3 to R004.6 served receipts: ``TriageDefect`` over the wire.

L4: a defect, a symbol, a changeset and its ``changes`` edge are seeded through
``BatchUpdate`` (no ``FileDefect`` call), the defect is triaged through
``TriageDefect``, and the decision, the ``affects`` edge and the committed record
are read back. An unknown or closed defect refuses and stores nothing.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import uuid
from pathlib import Path

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _decision_resident_fixture import publication
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient

FRAME = "at run (src/a.rs:15:3)"


def _defect(defect_id: str, status: str, text: str = FRAME) -> dict:
    record = {
        "id": defect_id,
        "repo": "demo",
        "title": "boom",
        "intake": "direct",
        "signature_digest": f"sig-{defect_id}",
        "severity": "medium",
        "status": status,
        "symptoms": [
            {
                "intake": "direct",
                "source_record_id": "src-1",
                "observed_at": 1000,
                "text": text,
            }
        ],
        "created_at": 900,
    }
    return {
        "op": "upsert_node",
        "id": f"defect:demo/{defect_id}",
        "properties": {
            "type": "Defect",
            "defect_record": json.dumps(record, separators=(",", ":")),
            "repo": "demo",
        },
    }


SEED = [
    _defect("d1", "new"),
    _defect("closed", "closed"),
    {
        "op": "upsert_node",
        "id": "symbol:a",
        "properties": {
            "type": "SYMBOL",
            "file_path": "src/a.rs",
            "line": "10",
            "end_line": "20",
            "qualified_symbol": "a::run",
        },
    },
    {
        "op": "upsert_node",
        "id": "commit:c1",
        "properties": {
            "type": "Changeset",
            "changeset_id": "c1",
            "repo": "demo",
            "author": "alice",
            "timestamp_ms": "500",
        },
    },
    {
        "op": "upsert_edge",
        "source": "commit:c1",
        "target": "symbol:a",
        "properties": {"relationship": "changes"},
    },
]


def _request(pin: dict, graph: str, defect_id: str, **extra) -> dict:
    request = {
        "tenant_id": TEST_TENANT,
        "graph": graph,
        "defect_id": defect_id,
        "as_of_ms": 1000,
        "culprit": None,
        "path_owners": ["alice"],
        "feature_schema": pin,
        "policy": {"policy": "default"},
    }
    request.update(extra)
    return request


async def _journey() -> dict:
    graph = f"defect-triage-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(SEED, use_bin_type=True)},
            graph,
        )
        try:
            entry = await client._send(
                "AgentComponent",
                publication(
                    "r004-3-schema",
                    "feature_schema",
                    ds.feature_schema_body([ds.feature("v", ds.number("v"), impute=0)]),
                ),
                graph,
            )
            pin = {
                "component_id": "r004-3-schema",
                "kind": "feature_schema",
                "definition_digest": entry["component"]["definition_digest"],
            }
            receipt = await client.triage_defect(_request(pin, graph, "d1"), graph)
            logged = await dc.DecisionClient(client=client).log(
                dc.get_op(TEST_TENANT, receipt["record_id"])
            )
            refusals = {}
            for defect_id in ("missing", "closed"):
                try:
                    await client.triage_defect(_request(pin, graph, defect_id), graph)
                except Exception as error:
                    refusals[defect_id] = str(error)
            return {"receipt": receipt, "logged": logged, "refusals": refusals}
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R004.3
@pytest.mark.spec("MEDULLA-SDLC-R004.3")
def test_triage_records_decision():
    journey = asyncio.run(_journey())
    decision = journey["receipt"]["decision"]
    assert decision["suspected_symbols"] == ["symbol:a"]
    assert decision["owner_principal"] == "alice"
    assert journey["receipt"]["edges_written"] == 1
    assert journey["logged"]["record"]["answer"]["type"] == "defect_triage"


# spec: MEDULLA-SDLC-R004.4
@pytest.mark.spec("MEDULLA-SDLC-R004.4")
def test_triage_refusals():
    journey = asyncio.run(_journey())
    assert "DEFECT_UNKNOWN" in journey["refusals"]["missing"]
    assert "DEFECT_CLOSED" in journey["refusals"]["closed"]


# spec: MEDULLA-SDLC-R004.5
@pytest.mark.spec("MEDULLA-SDLC-R004.5")
def test_triage_replays():
    journey = asyncio.run(_journey())
    facts = [p["fact"] for p in journey["logged"]["record"]["premises"]]
    assert "triage.defect" in facts
    assert "triage.blame:symbol:a=c1@500" in facts
    assert any(fact.startswith("knob:triage.max_frames=") for fact in facts)
    assert len(journey["logged"]["record"]["record_digest"]) == 64
