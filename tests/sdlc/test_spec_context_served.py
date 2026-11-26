"""Context competency questions (MEDULLA-SDLC-R014.15).

L4: one served `SpecQuestion` per context member over a seeded spec artifact
graph: task context, plan section, contract, seam routing and plan to goal and
phase. Each answers rows from the stored records and refuses an unknown subject
`COMPETENCY_SUBJECT_UNKNOWN`.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

from medulla.client import MedullaClient

SECTION = "plansection:r:specs/x/plan.md#L3"


def _node(node_id: str, node_type: str, **props: str) -> dict[str, Any]:
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {"type": node_type, **props},
    }


def _edge(source: str, relationship: str, target: str) -> dict[str, Any]:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


SEED = [
    _node("requirement:r:A-R1", "Requirement", repo="r", rowId="A-R1"),
    _node("requirement:r:A-R2", "Requirement", repo="r", rowId="A-R2"),
    _node(
        "requirement:p:P-R7",
        "Requirement",
        repo="p",
        rowId="P-R7",
        rowStatus="SPECIFIED",
    ),
    _node(
        "contract:r:seam",
        "Contract",
        seam="seam",
        provider_rows="p:P-R7",
        consumer_repos="r",
        wire_shape="{a}",
    ),
    _node("task:r:t#T1", "TaskLine", taskId="T1", phase="2", ticked="false"),
    _node(SECTION, "PlanSection", title="Phase 2", phase="2", digest="sha256:x"),
    _node("goal:g", "Goal", statement="ship"),
    _edge("task:r:t#T1", "forRow", "requirement:r:A-R1"),
    _edge("requirement:r:A-R1", "crossesSeam", "contract:r:seam"),
    _edge("requirement:p:P-R7", "crossesSeam", "contract:r:seam"),
    _edge(SECTION, "covers", "requirement:r:A-R1"),
    _edge(SECTION, "covers", "requirement:r:A-R2"),
    _edge("requirement:r:A-R1", "targetsGoal", "goal:g"),
]


def _body(payload: Any) -> dict[str, Any]:
    if isinstance(payload, (bytes, bytearray)):
        return dict(msgpack.unpackb(payload, raw=False))
    return dict(payload)


async def _ask(graph: str) -> dict[str, dict[str, Any]]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        try:
            await client._send(
                "BatchUpdate",
                {"operations_msgpack": msgpack.packb(SEED, use_bin_type=True)},
                graph,
            )
            asked = {
                "task_context": "task:r:t#T1",
                "plan_section": SECTION,
                "contract": "contract:r:seam",
                "seam_routing": "requirement:r:A-R1",
                "plan_to_goal_phase": SECTION,
            }
            answers = {
                question: _body(await client.spec_question(question, graph, subject))
                for question, subject in asked.items()
            }
            with pytest.raises(Exception, match="COMPETENCY_SUBJECT_UNKNOWN"):
                await client.spec_question("contract", graph, "contract:none")
            return answers
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R014.15")
def test_context_questions_answer_from_the_stored_records() -> None:
    # spec: MEDULLA-SDLC-R014.15
    answers = asyncio.run(_ask(f"sdlc-context-{uuid.uuid4().hex[:8]}"))

    task = answers["task_context"]["rows"][0]
    assert task["row"] == "requirement:r:A-R1"
    assert task["plan_sections"] == SECTION
    assert task["blocked_by_providers"] == "requirement:p:P-R7"
    assert task["blocked"] == "true"

    section = answers["plan_section"]["rows"][0]
    assert section["phase"] == "2"
    assert section["requirements"] == "requirement:r:A-R1,requirement:r:A-R2"

    seam = answers["contract"]["rows"][0]
    assert seam["wire_shape"] == "{a}"
    assert seam["unverified_providers"] == "requirement:p:P-R7"

    route = answers["seam_routing"]["rows"][0]
    assert (route["direction"], route["provider_repo"]) == ("consumer", "p")
    assert route["provider_row"] == "P-R7"

    goals = answers["plan_to_goal_phase"]["rows"]
    assert [row["goals"] for row in goals] == ["goal:g", ""]
    assert {row["phase"] for row in goals} == {"2"}
    assert all(answer["digests"] for answer in answers.values())
