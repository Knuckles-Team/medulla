"""Goal record (MEDULLA-SDLC-R017.1).

L3: every call goes through the running engine. A goal names its project, a
statement and targets (rows or competency questions); each target gets a
`targetsGoal` edge to the goal. A goal with no target refuses
`GOAL_TARGET_REQUIRED` and writes nothing.
"""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from _landing_fixture import LandingRepo, add_node, cited_fact, content_digest
from conftest import TEST_AGENT_ID, request_context

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


async def _set_mode(
    client, project: str, mode: str, approval: dict | None = None
) -> dict:
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

        absent = {
            "kind": "code_graph_fact",
            "record_id": f"fact:absent-{suffix}",
            "digest": digest,
        }
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

        # A development codebase takes a requested proposal, not an unprompted one.
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


async def _list(client, request: dict) -> list[dict]:
    return _body(await client._send("ProposalList", {"request": request}, GRAPH))


async def _decide(client, proposal_id: str, decision: str) -> dict:
    request = {
        "id": proposal_id,
        "decision": decision,
        "reviewer": "reviewer-1",
        "reason": f"{decision} after reading the evidence",
    }
    return _body(await client._send("ProposalDecide", {"request": request}, GRAPH))


@pytest.mark.spec("MEDULLA-SDLC-R017.6")
@pytest.mark.asyncio
async def test_review_list_and_decision_round_trip(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        cited = await _cited(client, suffix)
        await _set_mode(client, project, "development")
        first = await _proposal(client, project, [cited])

        listed = await _list(client, {"repo": project})
        assert [entry["id"] for entry in listed] == [first["node_id"]]
        entry = listed[0]
        assert entry["kind"] == "row_add"
        assert entry["target"] == "MEDULLA-SDLC-R017.3"
        assert entry["evidence"] == [cited]
        assert entry["state"] == "open"
        # The list narrows by state and kind, and never crosses projects.
        assert await _list(client, {"repo": project, "state": "approved"}) == []
        assert await _list(client, {"repo": project, "kind": "row_update"}) == []
        assert await _list(client, {"repo": f"other-{suffix}"}) == []

        # spec: MEDULLA-SDLC-R017.6
        decided = await _decide(client, first["node_id"], "approve")
        assert decided == {"id": first["node_id"], "state": "approved"}
        listed = await _list(client, {"repo": project, "state": "approved"})
        assert [entry["id"] for entry in listed] == [first["node_id"]]
        assert await _list(client, {"repo": project, "state": "open"}) == []
        stored = await _node(client, first["node_id"])
        assert stored["reviewer"] == "reviewer-1"
        assert stored["evidence"]

        # A decided proposal is not decided again; an unknown id is not found.
        with pytest.raises(Exception, match="PROPOSAL_NOT_OPEN"):
            await _decide(client, first["node_id"], "reject")
        with pytest.raises(Exception, match="PROPOSAL_NOT_FOUND"):
            await _decide(client, f"proposal:absent-{suffix}", "reject")

        # A rejection ends a second proposal the same way.
        second_cited = await _cited(client, suffix + "b")
        second = _body(
            await client._send(
                "ProposalPut",
                {
                    "proposal": {
                        "project": project,
                        "kind": "row_update",
                        "target": "MEDULLA-SDLC-R017.6",
                        "statement": "Update the review row.",
                        "evidence": [second_cited],
                    }
                },
                GRAPH,
            )
        )
        rejected = await _decide(client, second["node_id"], "reject")
        assert rejected["state"] == "rejected"
    finally:
        await client.close()


LOOP_AGENT = "agent:spec-loop"
SPEC_PATH = "specs/demo/requirements.md"


async def _connect_as(agent_id: str, principal: str):
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent_id, principal=principal),
    )


@pytest.mark.spec("MEDULLA-SDLC-R017.4")
@pytest.mark.asyncio
async def test_proposal_lands_only_through_changeset_and_gates(
    start_medulla_server,
) -> None:
    person = await _connect()
    loop = await _connect_as(LOOP_AGENT, TEST_AGENT_ID)
    suffix = uuid.uuid4().hex[:10]
    graph = f"proposal-landing-{suffix}"
    repo = None
    try:
        repo = await LandingRepo.create(person, graph, LOOP_AGENT)
        await person._send(
            "ProjectModeSet",
            {"request": {"project": graph, "mode": "development"}},
            graph,
        )
        cited = await cited_fact(person, graph, suffix)

        # A proposal names a row, never a spec file.
        with pytest.raises(Exception, match="PROPOSAL_DIRECT_WRITE_REFUSED"):
            await person._send(
                "ProposalPut",
                {
                    "proposal": {
                        "project": graph,
                        "kind": "row_update",
                        "target": SPEC_PATH,
                        "statement": "Rewrite the file.",
                        "evidence": [cited],
                    }
                },
                graph,
            )
        proposal = _body(
            await person._send(
                "ProposalPut",
                {
                    "proposal": {
                        "project": graph,
                        "kind": "row_update",
                        "target": "DEMO-R001",
                        "statement": "Tighten the row.",
                        "evidence": [cited],
                    }
                },
                graph,
            )
        )
        proposal_id = proposal["node_id"]
        spec = b"| `DEMO-R001` | one |\n"

        # The loop writes no spec file directly: no proposal, an open one and an
        # unknown one all refuse, and nothing lands.
        message = "feat: edit the row\n\nSpec: MEDULLA-SDLC-R017.4"
        await repo.branch("loop/direct")
        for trailer in (
            "",
            f"\nProposal: {proposal_id}",
            "\nProposal: proposal:absent",
        ):
            with pytest.raises(Exception, match="PROPOSAL_DIRECT_WRITE_REFUSED"):
                await repo.submit(
                    loop, "loop/direct", SPEC_PATH, spec, message + trailer
                )
        # A person edits the spec tree as any author does; the loop edits other
        # paths freely.
        await repo.branch("person/direct")
        edited = await repo.submit(person, "person/direct", SPEC_PATH, spec, message)
        assert edited["changeset"]["id"]
        await repo.submit(loop, "loop/direct", "notes.txt", b"note", message)

        # An approved proposal lets the loop deliver it as a changeset.
        decided = _body(
            await person._send(
                "ProposalDecide",
                {
                    "request": {
                        "id": proposal_id,
                        "decision": "approve",
                        "reviewer": "reviewer-1",
                        "reason": "the evidence holds",
                    }
                },
                graph,
            )
        )
        assert decided["state"] == "approved"
        delivery = (
            f"feat: edit the row\n\nSpec: MEDULLA-SDLC-R017.4\nProposal: {proposal_id}"
        )
        await repo.branch("loop/delivery")
        receipt = await repo.submit(loop, "loop/delivery", SPEC_PATH, spec, delivery)
        assert receipt["changeset"]["author_principal"] == TEST_AGENT_ID

        # The change reaches main only through the gate: no verdict, no landing.
        with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
            await repo.merge("loop/delivery")
        await repo.record(receipt, "loop/delivery")
        merged = await repo.merge("loop/delivery")
        assert merged["outcome"] == "fast_forward"

        # The merge of its pull request lands the proposal, not the merge to main.
        stored = _body(
            await person._send("GetNodeProperties", {"node_id": proposal_id}, graph)
        )
        assert stored["status"] == "approved"
        await person._send(
            "BatchUpdate",
            {
                "operations_msgpack": msgpack.packb(
                    [
                        {
                            "op": "upsert_node",
                            "id": "connector:scm",
                            "properties": {"type": "Connector"},
                        }
                    ],
                    use_bin_type=True,
                )
            },
            graph,
        )
        merged_event = {
            "event_id": f"merged-{suffix}",
            "connector": "scm",
            "kind": "proposal_merged",
            "source": "person",
            "proposal_id": proposal_id,
            "failed": False,
            "summary": "merged",
            "observed_at": 1,
        }
        await person.delivery_feedback(merged_event, graph)
        stored = _body(
            await person._send("GetNodeProperties", {"node_id": proposal_id}, graph)
        )
        # spec: MEDULLA-SDLC-R017.4
        assert stored["status"] == "landed"
        listed = _body(
            await person._send(
                "ProposalList", {"request": {"repo": graph, "state": "landed"}}, graph
            )
        )
        assert [entry["id"] for entry in listed] == [proposal_id]
    finally:
        if repo is not None:
            await repo.close()
        await loop.close()
        await person.close()


@pytest.mark.spec("MEDULLA-SDLC-R017.7")
@pytest.mark.asyncio
async def test_goal_drives_typed_proposal_to_pull_request(start_medulla_server) -> None:
    person = await _connect()
    loop = await _connect_as(LOOP_AGENT, TEST_AGENT_ID)
    suffix = uuid.uuid4().hex[:10]
    graph = f"goal-journey-{suffix}"
    repo = None
    try:
        repo = await LandingRepo.create(person, graph, LOOP_AGENT)
        approval = {
            "approver": "owner",
            "approved_at_ms": 1_700_000_000_000,
            "reason": "the owner promotes the project to evolving",
        }
        await person._send(
            "ProjectModeSet",
            {"request": {"project": graph, "mode": "evolving", "approval": approval}},
            graph,
        )

        # The goal targets one row of the repository.
        row = "DEMO-R001"
        await add_node(
            person, graph, f"requirement:{graph}:{row}", {"type": "Requirement"}
        )
        goal = _body(
            await person._send(
                "GoalPut",
                {
                    "goal": {
                        "project": graph,
                        "statement": "Land the demo row",
                        "targets": [{"kind": "row", "repo": graph, "id": row}],
                    }
                },
                graph,
            )
        )

        # A task fails its gate: the verdict is sealed and a defect opens.
        await repo.branch("loop/first")
        first = await repo.submit(
            loop,
            "loop/first",
            "src/demo.txt",
            b"first try",
            f"feat: first try\n\nSpec: {row}",
        )
        verdict = await repo.record(first, "loop/first", verdict="fail")
        with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
            await repo.merge("loop/first")
        defects = await person._send(
            "GetNodesByLabel", {"label": "Defect", "after": None, "limit": 10}, graph
        )
        assert defects, "the failed gate opened a defect"

        # The loop raises a split proposal from the failure, citing the sealed verdict.
        verdict_properties = _body(
            await person._send(
                "GetNodeProperties", {"node_id": verdict["node_id"]}, graph
            )
        )
        cited = [
            {
                "kind": "gate_result",
                "record_id": verdict["node_id"],
                "digest": content_digest(verdict_properties),
            }
        ]
        raised = _body(
            await loop._send(
                "ProposalPut",
                {
                    "proposal": {
                        "project": graph,
                        "kind": "task_split",
                        "target": "T001",
                        "statement": "Split the task that failed its gate.",
                        "evidence": cited,
                        "origin": "unprompted",
                    }
                },
                graph,
            )
        )
        reviewed = _body(
            await person._send(
                "ProposalDecide",
                {
                    "request": {
                        "id": raised["node_id"],
                        "decision": "approve",
                        "reviewer": "reviewer-1",
                        "reason": "the failure is repeated",
                    }
                },
                graph,
            )
        )
        assert reviewed["state"] == "approved"

        # The split lands as a changeset that names the proposal, through the gate.
        message = f"feat: split the task\n\nSpec: {row}\nProposal: {raised['node_id']}"
        receipt, merged = await repo.land(
            loop,
            "loop/split",
            "specs/demo/tasks.md",
            b"- [ ] T001a first half\n- [ ] T001b second half\n",
            message,
        )
        assert merged["outcome"] == "fast_forward"

        # The goal reads its progress: the landed changeset cites the targeted row.
        progress = await person.spec_question(
            "changes_advancing_goal", graph, goal["node_id"]
        )
        # spec: MEDULLA-SDLC-R017.7
        changesets = {answer["changeset"] for answer in progress["rows"]}
        assert any(node.endswith(receipt["changeset"]["id"]) for node in changesets), (
            progress
        )
    finally:
        if repo is not None:
            await repo.close()
        await loop.close()
        await person.close()
