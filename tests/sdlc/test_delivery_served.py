"""MEDULLA-SDLC-R019.3 to R019.14: the served delivery methods.

L3: every call goes through the running engine. Deliveries are opened through
``DeliveryOpen``; the connector records feedback needs are seeded in a batch.
"""

from __future__ import annotations

import asyncio
import hashlib
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


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _open_request(**overrides) -> dict:
    request = {
        "repo": "demo",
        "slice_root": "DEMO-R001",
        "scope_rows": ["DEMO-R001.1"],
        "dispatch_decision_id": "dec-1",
        "policy_version": "p1",
        "origin": "requested",
        "run_trace_id": "run-1",
        "evidence_digests": [],
    }
    request.update(overrides)
    return request


async def _with_graph(seed: list[dict], body):
    graph = f"sdlc-delivery-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        if seed:
            await client._send(
                "BatchUpdate",
                {"operations_msgpack": msgpack.packb(seed, use_bin_type=True)},
                graph,
            )
        try:
            return await body(client, graph)
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


async def _set_mode(client, graph: str, mode: str) -> None:
    await client._send(
        "ProjectModeSet",
        {"request": {"project": "demo", "mode": mode, "approval": None}},
        graph,
    )


# spec: MEDULLA-SDLC-R019.14
@pytest.mark.spec("MEDULLA-SDLC-R019.14")
def test_every_delivery_method_is_served_and_scoped():
    async def body(client, graph):
        await _set_mode(client, graph, "development")
        opened = _body(await client.delivery_open(_open_request(), graph))
        delivery = opened["delivery"]
        assert delivery["assignments"][0]["state"] == "pending"
        again = _body(await client.delivery_open(_open_request(), graph))
        assert again["reopened"] and again["delivery"]["id"] == delivery["id"]
        status = _body(await client.delivery_status(delivery["id"], graph))
        assert status["delivery"]["id"] == delivery["id"] and status["stage"]
        listed = _body(await client.delivery_list(graph))
        assert [row["id"] for row in listed["deliveries"]] == [delivery["id"]]
        second = _body(
            await client.delivery_open(
                _open_request(slice_root="DEMO-R002", run_trace_id="run-2"), graph
            )
        )
        assert second["delivery"]["id"] != delivery["id"]
        stopped = _body(await client.delivery_stop(delivery["id"], graph))
        assert stopped["delivery"]["ended"] == "stopped"
        with pytest.raises(Exception, match="DELIVERY_NOT_FOUND"):
            await client.delivery_status("delivery:unknown", graph)
        with pytest.raises(Exception, match="DELIVERY_SCOPE_EMPTY"):
            await client.delivery_open(_open_request(scope_rows=[]), graph)

    asyncio.run(_with_graph([], body))


# spec: MEDULLA-SDLC-R019.4
@pytest.mark.spec("MEDULLA-SDLC-R019.4")
def test_unprompted_slice_is_recorded_not_run_in_development_mode():
    async def body(client, graph):
        await _set_mode(client, graph, "development")
        shadow = _body(
            await client.delivery_open(_open_request(origin="unprompted"), graph)
        )
        assert shadow["delivery"] is None
        assert shadow["would_dispatch"]["rows"] == ["DEMO-R001.1"]
        assert _body(await client.delivery_list(graph))["deliveries"] == []
        await _set_mode(client, graph, "reference")
        with pytest.raises(Exception, match="MODE_REFERENCE_NO_PROPOSALS"):
            await client.delivery_open(_open_request(), graph)

    asyncio.run(_with_graph([], body))


# spec: MEDULLA-SDLC-R019.5
@pytest.mark.spec("MEDULLA-SDLC-R019.5")
def test_failed_check_reaches_the_delivery():
    seed = [
        {
            "op": "upsert_node",
            "id": "connector:scm",
            "properties": {"type": "Connector"},
        }
    ]

    async def body(client, graph):
        await _set_mode(client, graph, "development")
        opened = _body(await client.delivery_open(_open_request(), graph))
        delivery_id = opened["delivery"]["id"]
        assignment = opened["delivery"]["assignments"][0]["id"]
        await client.delivery_report(
            {
                "delivery_id": delivery_id,
                "assignment_id": assignment,
                "state": "waiting",
                "summary": "opened",
                "proposal": "pr-1",
                "question": None,
                "evidence": [],
            },
            graph,
        )
        event = {
            "event_id": "ev-1",
            "connector": "scm",
            "kind": "check_changed",
            "source": "automation",
            "proposal_id": "pr-1",
            "failed": True,
            "summary": "ci failed",
            "observed_at": 1,
        }
        receipt = _body(await client.delivery_feedback(event, graph))
        assert receipt["delivered_to"] == [delivery_id] and not receipt["duplicate"]
        again = _body(await client.delivery_feedback(event, graph))
        assert again["duplicate"] and again["delivered_to"] == []
        unknown = {**event, "event_id": "ev-2", "proposal_id": "pr-unknown"}
        with pytest.raises(Exception, match="FEEDBACK_PROPOSAL_UNKNOWN"):
            await client.delivery_feedback(unknown, graph)
        stranger = {**event, "event_id": "ev-3", "connector": "unregistered"}
        with pytest.raises(Exception, match="FEEDBACK_SOURCE_UNKNOWN"):
            await client.delivery_feedback(stranger, graph)

    asyncio.run(_with_graph(seed, body))


CHANGESET = "c" * 64
HEAD_OLD = "sha256:head-old"
HEAD_NEW = "sha256:head-new"


def _verdict_node(gate: str) -> dict:
    record = {
        "changeset": CHANGESET,
        "gate_id": gate,
        "input_digest": "a" * 64,
        "verdict": "pass",
        "evidence_digest": "b" * 64,
        "duration_ms": 1,
    }
    text = json.dumps(record, separators=(",", ":"))
    return {
        "op": "upsert_node",
        "id": f"gate_verdict:{CHANGESET}:{gate}",
        "properties": {
            "type": "GateVerdict",
            "verdict_record": text,
            "verdict_seal": hashlib.sha256(text.encode()).hexdigest(),
        },
    }


def _node(node_id: str, **properties) -> dict:
    return {"op": "upsert_node", "id": node_id, "properties": properties}


def _arming_request(schema: dict, graph: str, delivery_id: str, lines: int) -> dict:
    params = [
        dc.param("graph", "text", graph),
        dc.param("delivery_id", "text", delivery_id),
        dc.param("proposal_id", "text", "pr-1"),
        dc.param("head_commit", "text", HEAD_NEW),
        dc.param("head_changeset", "text", CHANGESET),
        dc.param("branch", "text", "main"),
        dc.param("required_check", "text", "ci"),
        dc.param("changed_lines", "int", lines),
    ]
    return dc.decide_request(
        TEST_TENANT,
        ("delivery.arming", "merge_arming", "ordinary"),
        dc.declared([dc.DeclaredOption("merge_arming")]),
        schema,
        params=params,
    )


def _chosen(record: dict) -> str:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return [s["option_id"] for s in outcome["scores"] if s["score"]["value"] != 0][0]


async def _arm(client, graph: str, schema: dict, delivery_id: str) -> dict:
    decision = dc.DecisionClient(client=client)
    payload = await decision.decide(_arming_request(schema, graph, delivery_id, 10))
    return payload["records"][0]


# spec: MEDULLA-SDLC-R019.15
@pytest.mark.spec("MEDULLA-SDLC-R019.15")
def test_slice_to_merged_proposal_through_feedback():
    seed = [
        _node("connector:scm", type="Connector"),
        _node(
            "landing_policy:demo:main",
            type="LandingPolicy",
            policy_record=json.dumps(
                {"repo": "demo", "branch": "main", "required_gates": ["unit"]},
                separators=(",", ":"),
            ),
        ),
        _verdict_node("unit"),
        _verdict_node("ci"),
        _node(
            "review:0",
            type="Review",
            proposal_id="pr-1",
            reviewer="alice",
            head_commit=HEAD_OLD,
            recorded_at_ms=1,
            state="approved",
        ),
    ]

    async def body(client, graph):
        await _set_mode(client, graph, "development")
        schema = await client._send(
            "AgentComponent",
            publication(
                "r019-15-schema",
                "feature_schema",
                ds.feature_schema_body([ds.feature("v", ds.number("v"), impute=0)]),
            ),
            graph,
        )
        pin = {
            "component_id": "r019-15-schema",
            "kind": "feature_schema",
            "definition_digest": _body(schema)["component"]["definition_digest"],
        }
        opened = _body(await client.delivery_open(_open_request(), graph))
        delivery_id = opened["delivery"]["id"]
        assignment = opened["delivery"]["assignments"][0]["id"]
        report = {
            "delivery_id": delivery_id,
            "assignment_id": assignment,
            "state": "ready",
            "summary": "ready for review",
            "proposal": "pr-1",
            "question": None,
            "evidence": [f"gate_verdict:{CHANGESET}:unit"],
        }
        ready = _body(await client.delivery_report(report, graph))
        assert ready["stage"] == "person/review"
        # A failed check arrives and becomes an input of the delivery.
        failed = {
            "event_id": "ev-check",
            "connector": "scm",
            "kind": "check_changed",
            "source": "automation",
            "proposal_id": "pr-1",
            "failed": True,
            "summary": "ci failed",
            "observed_at": 1,
        }
        receipt = _body(await client.delivery_feedback(failed, graph))
        assert receipt["delivered_to"] == [delivery_id]
        # The head moved since the last review: the verdict is disarm.
        record = await _arm(client, graph, pin, delivery_id)
        assert _chosen(record) == "verdict:disarm"
        # A pass and an owner approval on the new head arm.
        await client._send(
            "BatchUpdate",
            {
                "operations_msgpack": msgpack.packb(
                    [
                        _node(
                            "review:1",
                            type="Review",
                            proposal_id="pr-1",
                            reviewer="alice",
                            head_commit=HEAD_NEW,
                            recorded_at_ms=7,
                            state="approved",
                        ),
                        _node(
                            "approval:1",
                            type="OwnerApproval",
                            proposal_id="pr-1",
                            head_commit=HEAD_NEW,
                            recorded_at_ms=8,
                        ),
                    ],
                    use_bin_type=True,
                )
            },
            graph,
        )
        armed = await _arm(client, graph, pin, delivery_id)
        assert _chosen(armed) == "verdict:arm"
        # The decision is a record of the log and replays from it.
        decision = dc.DecisionClient(client=client)
        committed = await decision.log(dc.commit_op(armed))
        assert committed["record_digest"] == armed["record_digest"]
        # A merge event ends the delivery as merged.
        merged = {
            **failed,
            "event_id": "ev-merge",
            "kind": "proposal_merged",
            "source": "person",
            "failed": False,
            "summary": "merged",
        }
        await client.delivery_feedback(merged, graph)
        status = _body(await client.delivery_status(delivery_id, graph))
        assert status["delivery"]["ended"] == "merged"
        assert status["stage"] == "ended"

    asyncio.run(_with_graph(seed, body))
