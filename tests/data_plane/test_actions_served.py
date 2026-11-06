"""Served governed actions (MEDULLA-UNIFIED-DATA-PLANE-R700.3 / R700.6).

`ActionRegister` stores a validated spec; `ActionSubmit` applies every effect
and records the run in one batch. A failing criterion applies no effect and
records a refused run.
"""

from __future__ import annotations

import json
import os
import uuid

import msgpack
import pytest
from conftest import request_context

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _spec(name: str, account: str) -> dict:
    return {
        "name": name,
        "params": [
            {"name": "amount", "ty": "integer", "required": True},
            {"name": "ref", "ty": "string", "required": True},
        ],
        "criteria": [
            {"name": "positive", "param": "amount", "op": "gt", "value": 0},
            {"name": "under_cap", "param": "amount", "op": "le", "value": 100},
        ],
        "effects": [
            {
                "kind": {"kind": "create_node"},
                "template": (
                    '{"id": {{ref}}, "label": "Refund", '
                    '"properties": {"amount": {{amount}}}}'
                ),
            },
            {
                "kind": {"kind": "set_property"},
                "template": (
                    '{"node": "'
                    + account
                    + '", "property": "refunded", "value": {{amount}}}'
                ),
            },
        ],
    }


async def _connect():
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _node(client, node_id: str):
    try:
        raw = await client._send("GetNodeProperties", {"node_id": node_id}, GRAPH)
    except Exception:
        return None
    return _body(raw)


async def _feed_head(client) -> int:
    """The next sequence the feed will assign: the shared graph's ring can hold
    more than one default page, so the cursor is the head, never a page length."""
    _events, _watermark, head, _epoch = await client.streaming.cdc_read_with_watermark(
        GRAPH, 0, limit=1
    )
    return head


async def _feed_after(client, head: int) -> list:
    """Every event from ``head`` on, page by page."""
    events: list = []
    cursor = head
    while page := await client.streaming.cdc_read(GRAPH, cursor):
        events.extend(page)
        cursor = page[-1]["seq"] + 1
    return events


async def _registered(client) -> tuple[str, str]:
    suffix = uuid.uuid4().hex[:8]
    name, account = f"approve_refund_{suffix}", f"acct-{suffix}"
    registered = _body(
        await client._send("ActionRegister", {"spec": _spec(name, account)}, GRAPH)
    )
    assert registered == name
    return name, account


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R700.3")
@pytest.mark.asyncio
async def test_register_submit_and_outbox_event(start_medulla_server) -> None:
    client = await _connect()
    try:
        name, account = await _registered(client)
        cursor = await _feed_head(client)
        run = _body(
            await client._send(
                "ActionSubmit",
                {"action": name, "params": {"amount": 40, "ref": f"refund-{name}"}},
                GRAPH,
            )
        )
        assert run["status"] == "applied"
        assert [v["passed"] for v in run["verdicts"]] == [True, True]
        events = await _feed_after(client, cursor)
        assert any(e["node_id"] == "__action_completed" for e in events)
        assert any(str(e["node_id"]).startswith("action-run:") for e in events)
        assert (await _node(client, account))["refunded"] == 40
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R700.6")
@pytest.mark.asyncio
async def test_failed_criterion_applies_no_effect(start_medulla_server) -> None:
    client = await _connect()
    try:
        name, account = await _registered(client)
        ref = f"refund-{name}"
        with pytest.raises(Exception, match="ACTION_CRITERION_FAILED"):
            await client._send(
                "ActionSubmit",
                {"action": name, "params": {"amount": 500, "ref": ref}},
                GRAPH,
            )
        assert await _node(client, ref) is None
        assert await _node(client, account) is None
        runs = [
            e
            for e in await client.streaming.cdc_read(GRAPH, 0)
            if str(e["node_id"]).startswith("action-run:") and e.get("after")
        ]
        recorded = [json.loads(_body(e["after"])["action_run"]) for e in runs]
        refused = [r for r in recorded if r["action"] == name]
        assert [v["passed"] for v in refused[-1]["verdicts"]] == [True, False]
        assert refused[-1]["status"] == "refused"

        run = _body(
            await client._send(
                "ActionSubmit",
                {"action": name, "params": {"amount": 60, "ref": ref}},
                GRAPH,
            )
        )
        assert run["status"] == "applied"
        assert (await _node(client, ref))["amount"] == 60
        assert (await _node(client, account))["refunded"] == 60
    finally:
        await client.close()
