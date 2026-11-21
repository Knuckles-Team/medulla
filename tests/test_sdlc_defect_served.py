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


async def _add_node(client: MedullaClient, graph: str, node_id: str, props: dict):
    from medulla.generated import graph as generated_graph

    await generated_graph.send_add_node(
        client,
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


async def _run_trace_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        failed = {
            "node_type": "RunTrace",
            "run_trace_id": "trace-a",
            "runTaskClass": "fix",
        }
        # A finished run files nothing; a failed one opens a defect of its own.
        await _add_node(
            client,
            graph,
            "run:ok",
            {**failed, "run_trace_id": "trace-ok", "terminationReason": "final_answer"},
        )
        absent = await client._send(
            "Uql", {"text": "MATCH (:Defect)", "params": {}}, graph=graph
        )
        assert absent["rows"] == []
        await _add_node(
            client, graph, "run:a", {**failed, "terminationReason": "step_budget"}
        )
        defect = await _defect(client, graph, "def-0001")
        assert defect["intake"] == "run_failure" and defect["symptom_count"] == 1
        record = json.loads(defect["defect_record"])
        assert record["symptoms"][0]["source_record_id"] == "trace-a"
        # The same failure of the next run lands on the same defect.
        await _add_node(
            client,
            graph,
            "run:b",
            {**failed, "run_trace_id": "trace-b", "terminationReason": "step_budget"},
        )
        assert (await _defect(client, graph, "def-0001"))["symptom_count"] == 2
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.3
@pytest.mark.spec("MEDULLA-SDLC-R003.3")
def test_failed_run_trace_opens_defect():
    asyncio.run(_run_trace_scenario(f"sdlc-defect-run-{uuid.uuid4().hex[:8]}"))


async def _incident_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        await _add_node(
            client,
            graph,
            "symbol:handler",
            {
                "type": "SYMBOL",
                "file_path": "src/handler.rs",
                "line": "10",
                "end_line": "40",
                "qualified_symbol": "Handler::serve",
            },
        )
        reading = {
            "entity_ref": "panic at src/handler.rs:22:5",
            "signal": "error_rate",
            "observed": 9.0,
            "baseline": 1.0,
            "at_ms": 10,
        }
        recorded = _body(
            await client._send(
                "IncidentRecordAnomaly",
                {"anomaly": reading, "window_ms": 60_000},
                graph,
            )
        )
        assert recorded["incident_ids"]
        defect = await _defect(client, graph, "def-0001")
        assert defect["intake"] == "incident" and defect["symptom_count"] == 1
        record = json.loads(defect["defect_record"])
        assert "Handler::serve" in record["symptoms"][0]["text"]
        # A replayed reading adds no evidence, so it files nothing.
        await client._send(
            "IncidentRecordAnomaly", {"anomaly": reading, "window_ms": 60_000}, graph
        )
        assert (await _defect(client, graph, "def-0001"))["symptom_count"] == 1
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.3
@pytest.mark.spec("MEDULLA-SDLC-R003.3")
def test_incident_naming_a_symbol_opens_defect():
    asyncio.run(_incident_scenario(f"sdlc-defect-inc-{uuid.uuid4().hex[:8]}"))


async def _file_direct(client: MedullaClient, graph: str, text: str) -> dict:
    return await client.file_defect(
        {
            "repo": graph,
            "intake": "direct",
            "title": "transition probe",
            "source_record_id": "record-1",
            "observed_at": 1000,
            "text": text,
        }
    )


async def _transition(client: MedullaClient, graph: str, to: str, **extra) -> dict:
    return await client.transition_defect(
        {"repo": graph, "defect_id": "def-0001", "to": to, **extra}
    )


async def _transition_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        await _file_direct(client, graph, "panic in transition probe")
        # A move that skips a state refuses and writes nothing.
        with pytest.raises(Exception, match="DEFECT_TRANSITION_ILLEGAL"):
            await _transition(client, graph, "verified")
        assert (await _defect(client, graph, "def-0001"))["status"] == "new"
        await _transition(client, graph, "triaged")
        await _transition(client, graph, "fixing")
        # The fix reference is a guard of the move to fixed.
        with pytest.raises(Exception, match="DEFECT_TRANSITION_ILLEGAL"):
            await _transition(client, graph, "fixed")
        fix, _, fix_digest = await _submit(client, graph, None, "fix")
        await _transition(client, graph, "fixed", fix_changeset=fix)
        # No verdict of the bound test yet: the move to verified is unverified.
        with pytest.raises(Exception, match="DEFECT_UNVERIFIED"):
            await _transition(client, graph, "verified", gate_id="bound-test")
        failing = _body(
            await client._send(
                "RecordGateVerdict",
                {
                    "changeset": fix,
                    "branch": "main",
                    "gate_id": "bound-test",
                    "input_digest": fix_digest,
                    "verdict": "fail",
                    "evidence_digest": hashlib.sha256(b"log").hexdigest(),
                    "duration_ms": 5,
                },
                graph,
            )
        )
        assert failing["node_id"]
        with pytest.raises(Exception, match="DEFECT_UNVERIFIED"):
            await _transition(client, graph, "verified", gate_id="bound-test")
        assert (await _defect(client, graph, "def-0001"))["status"] == "fixed"
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.4
@pytest.mark.spec("MEDULLA-SDLC-R003.4")
def test_illegal_transitions_refuse():
    asyncio.run(_transition_scenario(f"sdlc-defect-move-{uuid.uuid4().hex[:8]}"))


async def _verified_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        await _file_direct(client, graph, "panic in verified probe")
        fix, _, fix_digest = await _submit(client, graph, None, "fix")
        for to, extra in (
            ("triaged", {}),
            ("fixing", {}),
            ("fixed", {"fix_changeset": fix}),
        ):
            await _transition(client, graph, to, **extra)
        await client._send(
            "RecordGateVerdict",
            {
                "changeset": fix,
                "branch": "main",
                "gate_id": "bound-test",
                "input_digest": fix_digest,
                "verdict": "pass",
                "evidence_digest": hashlib.sha256(b"log").hexdigest(),
                "duration_ms": 5,
            },
            graph,
        )
        moved = await _transition(client, graph, "verified", gate_id="bound-test")
        assert moved["from"] == "fixed" and moved["to"] == "verified"
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.4
@pytest.mark.spec("MEDULLA-SDLC-R003.4")
def test_verified_move_needs_a_passing_bound_test():
    asyncio.run(_verified_scenario(f"sdlc-defect-ver-{uuid.uuid4().hex[:8]}"))


async def _readback_scenario(graph: str) -> None:
    client = await _client(graph)
    try:
        await _file_direct(client, graph, "panic in readback probe")
        await client.file_defect(
            {
                "repo": graph,
                "intake": "run_failure",
                "title": "transition probe",
                "source_record_id": "record-2",
                "observed_at": 2000,
                "text": "panic in readback probe",
            }
        )
        stored = json.loads((await _defect(client, graph, "def-0001"))["defect_record"])
        # A read through a second principal context equals the stored list.
        reader = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            timeout=60,
        )
        try:
            answer = await reader.get_defect({"repo": graph, "defect_id": "def-0001"})
        finally:
            await reader.close()
        assert answer["defect"]["symptoms"] == stored["symptoms"]
        assert [s["intake"] for s in answer["defect"]["symptoms"]] == [
            "direct",
            "run_failure",
        ]
        assert [s["source_record_id"] for s in answer["defect"]["symptoms"]] == [
            "record-1",
            "record-2",
        ]
        assert [s["observed_at"] for s in answer["defect"]["symptoms"]] == [1000, 2000]
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R003.5
@pytest.mark.spec("MEDULLA-SDLC-R003.5")
def test_symptom_evidence_reads_back():
    asyncio.run(_readback_scenario(f"sdlc-defect-read-{uuid.uuid4().hex[:8]}"))
