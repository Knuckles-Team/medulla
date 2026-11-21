"""MEDULLA-SDLC-R003.3 and R003.6: served defect intake.

A failed gate verdict opens a ``Defect`` node in the verdict's own batch; the same
failure on a
later changeset lands on the same defect. ``FileDefect`` deduplicates symptoms to
signatures.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid

import msgpack
import pytest
from conftest import TEST_AGENT_ID, request_context

from medulla.client import MedullaClient

MODE = 0o100644


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _listing_digest(entries: list[dict]) -> str:
    text = "".join(
        f"{e['path']}\t{e['blob_digest']}\t{e['mode']:o}\n"
        for e in sorted(entries, key=lambda e: e["path"])
    )
    return hashlib.sha256(text.encode()).hexdigest()


async def _client(graph: str) -> MedullaClient:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    await client.tenants.create(graph, "Global")
    await client._send(
        "SetPathGrant",
        {
            "request": {
                "repo": graph,
                "principal": TEST_AGENT_ID,
                "prefix": "",
                "read": True,
                "write": True,
            }
        },
    )
    return client


async def _submit(
    client: MedullaClient, graph: str, base_head, tag: str
) -> tuple[str, str, str]:
    """One changeset on main: its id, the branch head and the tree digest."""
    data = f"defect {tag} {uuid.uuid4().hex[:8]}".encode()
    digest = await client.blob.store(data)
    receipt = await client._send(
        "SubmitChangeset",
        {
            "request": {
                "repo": graph,
                "branch": "main",
                "base_head": base_head,
                "message": f"{tag}\n\nSpec: MEDULLA-SDLC-R003.3",
                "paths": [
                    {
                        "path": f"{tag}.txt",
                        "op": "add",
                        "blob": {"digest": digest, "size": len(data)},
                        "mode": MODE,
                        "renamed_from": None,
                    }
                ],
                "trailers": None,
                "run_trace_id": None,
            }
        },
        idempotency_key=f"defect-{graph}-{tag}",
    )
    tree = await client._send(
        "ReadTree",
        {
            "request": {
                "repo": graph,
                "branch": "main",
                "rev": receipt["branch_head"],
                "prefix": "",
                "depth": 0,
            }
        },
    )
    return (
        receipt["changeset"]["id"],
        receipt["branch_head"],
        _listing_digest(tree["entries"]),
    )


async def _fail_gate(client, graph, changeset, input_digest, gate="test") -> dict:
    return _body(
        await client._send(
            "RecordGateVerdict",
            {
                "changeset": changeset,
                "branch": "main",
                "gate_id": gate,
                "input_digest": input_digest,
                "verdict": "fail",
                "evidence_digest": hashlib.sha256(b"log").hexdigest(),
                "duration_ms": 5,
            },
            graph,
        )
    )


async def _defect(client, graph: str, defect_id: str) -> dict:
    return _body(
        await client._send(
            "GetNodeProperties", {"node_id": f"defect:{graph}/{defect_id}"}, graph
        )
    )


async def _gate_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        first, head, first_digest = await _submit(client, graph, None, "one")
        verdict = await _fail_gate(client, graph, first, first_digest)
        defect = await _defect(client, graph, "def-0001")
        assert defect["type"] == "Defect" and defect["status"] == "new"
        assert defect["intake"] == "gate_failure" and defect["symptom_count"] == 1
        record = json.loads(defect["defect_record"])
        assert record["symptoms"][0]["source_record_id"] == verdict["node_id"]
        assert record["symptoms"][0]["intake"] == "gate_failure"

        # A replayed verdict files nothing; the same failure on the next changeset
        # lands on
        # the same defect.
        await _fail_gate(client, graph, first, first_digest)
        assert (await _defect(client, graph, "def-0001"))["symptom_count"] == 1
        second, _, second_digest = await _submit(client, graph, head, "two")
        await _fail_gate(client, graph, second, second_digest)
        assert (await _defect(client, graph, "def-0001"))["symptom_count"] == 2
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.3
@pytest.mark.spec("MEDULLA-SDLC-R003.3")
def test_failed_gate_opens_defect():
    asyncio.run(_gate_scenario(f"sdlc-defect-gate-{uuid.uuid4().hex[:8]}"))


async def _dedupe_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        signatures, symptoms = 50, 1000
        outcomes = []
        for index in range(symptoms):
            receipt = await client.file_defect(
                {
                    "repo": graph,
                    "intake": "direct",
                    "title": f"signature {index % signatures}",
                    "source_record_id": f"record-{index}",
                    "observed_at": 1000 + index,
                    # Line numbers and addresses normalize away; the signature name
                    # stays.
                    "text": (
                        f"panic in module{index % signatures} "
                        f"at src/a.rs:{index} addr 0x{index:x}"
                    ),
                }
            )
            outcomes.append(receipt["outcome"])
        assert outcomes.count("created") == signatures
        assert outcomes.count("absorbed") == symptoms - signatures
        found = await client._send(
            "Uql", {"text": "MATCH (:Defect)", "params": {}}, graph=graph
        )
        assert len(found["rows"]) == signatures
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.6
@pytest.mark.spec("MEDULLA-SDLC-R003.6")
def test_symptoms_dedupe_to_signatures():
    asyncio.run(_dedupe_scenario(f"sdlc-defect-dedupe-{uuid.uuid4().hex[:8]}"))
