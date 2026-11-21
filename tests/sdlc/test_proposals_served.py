"""Goal record (MEDULLA-SDLC-R017.1).

L3: every call goes through the running engine. A goal names its project, a
statement and targets (rows or competency questions); each target gets a
`targetsGoal` edge to the goal. A goal with no target refuses
`GOAL_TARGET_REQUIRED` and writes nothing.
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


def _content_digest(properties: dict) -> str:
    from medulla.generated.digest import framed_sha256

    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return framed_sha256(b"eg/ingest-readback/node/v1", [compact.encode()])


async def _proposal(
    client, project: str, evidence: list[dict], origin: str | None = None
) -> dict:
    proposal = {
        "project": project,
        "kind": "row_add",
        "target": "MEDULLA-SDLC-R017.3",
        "statement": "Add the evidence row.",
        "evidence": evidence,
    }
    if origin is not None:
        proposal["origin"] = origin
    return _body(await client._send("ProposalPut", {"proposal": proposal}, GRAPH))


async def _set_mode(client, project: str, mode: str, approval: dict | None = None) -> dict:
    request: dict = {"project": project, "mode": mode}
    if approval is not None:
        request["approval"] = approval
    return _body(await client._send("ProjectModeSet", {"request": request}, GRAPH))


async def _cited(client, suffix: str) -> dict:
    record_id = f"fact:mode-{suffix}"
    properties = {"type": "CodeFact", "name": f"mode-{suffix}"}
    await _add_node(client, record_id, properties)
    return {
        "kind": "code_graph_fact",
        "record_id": record_id,
        "digest": _content_digest(properties),
    }


@pytest.mark.spec("MEDULLA-SDLC-R017.3")
@pytest.mark.asyncio
async def test_proposal_without_evidence_is_refused(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        await _set_mode(client, project, "development")
        with pytest.raises(Exception, match="PROPOSAL_EVIDENCE_REQUIRED"):
            await _proposal(client, project, [])

        record_id = f"fact:{suffix}"
        properties = {"type": "CodeFact", "name": f"fact-{suffix}"}
        await _add_node(client, record_id, properties)
        digest = _content_digest(properties)

        absent = {"kind": "code_graph_fact", "record_id": f"fact:absent-{suffix}", "digest": digest}
        with pytest.raises(Exception, match="PROPOSAL_EVIDENCE_UNRESOLVED"):
            await _proposal(client, project, [absent])
        wrong = {"kind": "code_graph_fact", "record_id": record_id, "digest": "0" * 64}
        with pytest.raises(Exception, match="PROPOSAL_EVIDENCE_DIGEST_MISMATCH"):
            await _proposal(client, project, [wrong])

        cited = {"kind": "code_graph_fact", "record_id": record_id, "digest": digest}
        receipt = await _proposal(client, project, [cited])
        # spec: MEDULLA-SDLC-R017.3
        assert receipt["evidence"] == 1
        stored = await _node(client, receipt["node_id"])
        assert stored["type"] == "Proposal"
        assert stored["status"] == "proposed"
        assert stored["origin"] == "requested"
        again = await _proposal(client, project, [cited])
        assert again["node_id"] == receipt["node_id"]
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R017.5")
@pytest.mark.asyncio
async def test_mode_gates_proposals(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        cited = await _cited(client, suffix)

        # A project with no mode record is a reference codebase: nothing is taken.
        with pytest.raises(Exception, match="MODE_REFERENCE_NO_PROPOSALS"):
            await _proposal(client, project, [cited])

        # A development codebase takes a requested proposal and refuses an unprompted one.
        await _set_mode(client, project, "development")
        receipt = await _proposal(client, project, [cited], origin="requested")
        assert receipt["evidence"] == 1
        with pytest.raises(Exception, match="MODE_ORIGIN_NOT_ADMITTED"):
            await _proposal(client, project, [cited], origin="unprompted")

        # An evolving codebase takes the proposal the loop raised unprompted.
        approval = {
            "approver": "owner",
            "approved_at_ms": 1_700_000_000_000,
            "reason": "the owner promotes the project to evolving",
        }
        await _set_mode(client, project, "evolving", approval)
        raised = await _proposal(client, project, [cited], origin="unprompted")
        # spec: MEDULLA-SDLC-R017.5
        assert (await _node(client, raised["node_id"]))["origin"] == "unprompted"

        # Proposals never change the mode.
        again = await _set_mode(client, project, "evolving")
        assert again["previous"] == "evolving"
    finally:
        await client.close()
