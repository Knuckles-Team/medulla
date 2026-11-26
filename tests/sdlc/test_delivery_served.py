"""MEDULLA-SDLC-R019.3 to R019.14: the served delivery methods.

L3: every call goes through the running engine. Deliveries are opened through
``DeliveryOpen``; the connector records feedback needs are seeded in a batch.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import msgpack
import pytest
from conftest import request_context

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
        with pytest.raises(Exception, match="FEEDBACK_PROPOSAL_UNKNOWN"):
            await client.delivery_feedback(event, graph)

    asyncio.run(_with_graph(seed, body))
