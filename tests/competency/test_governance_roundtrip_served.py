"""MEDULLA-CQ-ORCH-R701.6 served process model round trip.

L4: every call goes through the running engine over the wire client. A canonical
workflow in the form the ARIS process source produces (an EPC source with the AND, OR
and XOR gates, every step kind and an annex entry) is imported, then exported with
the target ``aris_epc``. The gates, the step kinds, the annex and the digest survive.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient

GRAPH = "__commons__"
SCHEMA = "medulla.process.canonical/v1"

# One chain holding every step kind and the three gates. A chain has exactly one
# allocation order, so the document is its own canonical form.
STEPS: list[dict[str, Any]] = [
    {"kind": "start", "label": "Request received"},
    {"kind": "human", "label": "Review request", "role": "Governance board"},
    {"kind": "agent", "label": "Collect evidence"},
    {"kind": "decision", "label": "Check policy", "ref": "policy-limits"},
    {"kind": "fork", "gate": "and", "label": "Open parallel checks"},
    {"kind": "branch", "gate": "xor", "label": "Risk class"},
    {"kind": "fork", "gate": "or", "label": "Notify owners"},
    {"kind": "join", "gate": "or", "label": "Owners notified"},
    {"kind": "wait", "label": "Await objection window"},
    {"kind": "call", "label": "Run exception process", "ref": "process:exception"},
    {"kind": "boundary", "label": "Escalation timer"},
    {"kind": "join", "gate": "and", "label": "Checks complete"},
    {"kind": "end", "label": "Decision recorded"},
]
ANNEX = [
    {
        "construct": "epc-attachment",
        "element_id": "attachment-1",
        "label": "Policy document",
        "after": "s3",
    }
]


def _canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _workflow() -> dict[str, Any]:
    steps = [{"id": f"s{n}", **step} for n, step in enumerate(STEPS, start=1)]
    flows = [
        {"source": f"s{n}", "target": f"s{n + 1}", "default": False}
        for n in range(1, len(steps))
    ]
    digest = hashlib.sha256(
        _canonical_json({"schema": SCHEMA, "steps": steps, "flows": flows}).encode()
    ).hexdigest()
    return {
        "schema": SCHEMA,
        "schema_version": 1,
        "name": "Governance review",
        "steps": steps,
        "flows": flows,
        "digest": digest,
        "source": {
            "schema_version": 1,
            "system": "aris_epc",
            "model_id": "governance-review",
            "elements": {step["id"]: step["id"] for step in steps},
        },
        "annex": ANNEX,
    }


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )


async def _round_trip() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    client = await _connect()
    try:
        workflow = _workflow()
        workflow_id = f"workflow:gov-{uuid.uuid4().hex[:8]}"
        imported = await client.graph.process_model_import(workflow, workflow_id)
        exported = await client.graph.process_model_export(
            workflow_id, imported["version"], target="aris_epc"
        )
        return workflow, imported, exported
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R701.6")
def test_roundtrip_preserves_gates(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R701.6
    workflow, imported, exported = asyncio.run(_round_trip())
    assert imported["version"] == 1
    assert imported["workflow_digest"] == "sha256:" + workflow["digest"]
    back = exported["workflow"]
    assert back["digest"] == workflow["digest"]
    assert [(s["id"], s.get("gate")) for s in back["steps"]] == [
        (s["id"], s.get("gate")) for s in workflow["steps"]
    ]
    assert {s.get("gate") for s in back["steps"]} >= {"and", "or", "xor"}
    assert [s["kind"] for s in back["steps"]] == [s["kind"] for s in workflow["steps"]]
    assert {s["kind"] for s in back["steps"]} == {
        "start",
        "end",
        "human",
        "agent",
        "decision",
        "branch",
        "fork",
        "join",
        "wait",
        "call",
        "boundary",
    }
    assert back["annex"] == ANNEX
