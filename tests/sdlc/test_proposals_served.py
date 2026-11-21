"""Goal record (MEDULLA-SDLC-R017.1).

L3: every call goes through the running engine. A goal names its project, a
statement and targets (rows or competency questions); each target gets a
`targetsGoal` edge to the goal. A goal with no target refuses
`GOAL_TARGET_REQUIRED` and writes nothing.
"""

from __future__ import annotations

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


async def _add_node(client, node_id: str, properties: dict) -> None:
    await client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        GRAPH,
    )


@pytest.mark.spec("MEDULLA-SDLC-R017.1")
@pytest.mark.asyncio
async def test_goal_needs_a_target(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        statement = f"Land the spec graph {suffix}"

        with pytest.raises(Exception, match="GOAL_TARGET_REQUIRED"):
            await _goal(client, project, statement, [])

        row_node = f"requirement:{project}:DEMO-R001"
        question_node = f"competency_question:cq-{suffix}"
        await _add_node(client, row_node, {"type": "Requirement"})
        await _add_node(client, question_node, {"type": "CompetencyQuestion"})
        targets = [
            {"kind": "row", "repo": project, "id": "DEMO-R001"},
            {"kind": "competency_question", "id": f"cq-{suffix}"},
        ]
        receipt = await _goal(client, project, statement, targets)
        # spec: MEDULLA-SDLC-R017.1
        assert receipt["targets"] == 2
        stored = await _node(client, receipt["node_id"])
        assert stored["type"] == "Goal"
        assert stored["project"] == project
        assert stored["statement"] == statement
    finally:
        await client.close()


async def _goal(client, project: str, statement: str, targets: list[dict]) -> dict:
    goal = {"project": project, "statement": statement, "targets": targets}
    return _body(await client._send("GoalPut", {"goal": goal}, GRAPH))
