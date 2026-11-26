"""MEDULLA-DECISION-ENGINE-R713.8 served skill telemetry.

L4: every call goes through the running engine over the wire client. A service
ingests 1,000 invocations from 30 pseudonyms, a mix of agent and user choice, in
batches of 500. The twelve measures of the skill match the figures computed from
the same events. Three skills rank, the run traces of a skill list by eval outcome,
and a caller whose computed cohort is 4 asks for peers and is refused with
``PEER_COHORT_TOO_SMALL``.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient

GRAPH = "__commons__"
DAY = 24 * 60 * 60 * 1000


def caller_principal_id(principal: object) -> str:
    """The persistence id the engine derives from a verified principal."""
    return "principal:sha256:" + hashlib.sha256(str(principal).encode()).hexdigest()


def _event(
    tenant: str, skill: str, index: int, user: int, run_id: str
) -> dict[str, Any]:
    day = index % 30
    return {
        "invocation_id": f"{skill}-{index}",
        "tenant_id": "ignored",
        "skill_ref": skill,
        "skill_version": "3",
        "chosen_by": "agent" if index % 4 else "user",
        "principal_pseudonym": dc.skill_pseudonym(tenant, f"user-{user}"),
        "run_id": run_id,
        "started_ms": day * DAY + index,
        "ended_ms": day * DAY + index + 100 + index % 50,
        "outcome": {"error": {"code": "TOOL_TIMEOUT"}} if index % 10 == 0 else "ok",
        "cost_delta_micro_usd": index % 7,
        "tokens_in": 10,
        "tokens_out": 20,
    }


async def _journey(prefix: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(scopes=["telemetry:write", "telemetry:read"]),
        graph_name=GRAPH,
    )
    try:
        tenant = request_context()["tenant"]
        skills = [f"skill:{prefix}-{name}@1" for name in ("a", "b", "c")]
        runs = [f"{prefix}-run-{n}" for n in range(3)]
        nodes: list[dict[str, Any]] = [
            {"op": "upsert_node", "id": skill, "properties": {"type": "Skill"}}
            for skill in skills
        ]
        for n, run_id in enumerate(runs):
            nodes.append(
                {
                    "op": "upsert_node",
                    "id": f"{prefix}-trace-{n}",
                    "properties": {"node_type": "RunTrace", "run_id": run_id},
                }
            )
        # Run 0 passed its eval, run 1 failed it, run 2 has no score.
        for n, value in ((0, 0.9), (1, 0.1)):
            nodes.append(
                {
                    "op": "upsert_node",
                    "id": f"{prefix}-score-{n}",
                    "properties": {
                        "node_type": "Score",
                        "value": value,
                        "range_min": 0.0,
                        "range_max": 1.0,
                    },
                }
            )
            nodes.append(
                {
                    "op": "upsert_edge",
                    "source": f"{prefix}-trace-{n}",
                    "target": f"{prefix}-score-{n}",
                    "properties": {"relationship": "scoredBy"},
                }
            )
        await client.lifecycle.batch_update(nodes)
        telemetry = dc.DecisionClient(client=client, graph=GRAPH)

        events = [
            _event(tenant, skills[0], index, index % 30, runs[index % 3])
            for index in range(1000)
        ]
        stored = 0
        for start in (0, 500):
            receipt = await telemetry.skill_telemetry(
                dc.skill_telemetry_ingest_op(GRAPH, events[start : start + 500])
            )
            stored += receipt["stored"]
        extra = [_event(tenant, skills[1], n, n % 5, runs[0]) for n in range(10)] + [
            _event(tenant, skills[2], n, n % 4, runs[1]) for n in range(10)
        ]
        # The caller used the third skill too, so four others share its class.
        caller = _event(tenant, skills[2], 10, 0, runs[1])
        caller["principal_pseudonym"] = dc.skill_pseudonym(
            tenant, caller_principal_id(request_context()["principal"])
        )
        extra.append(caller)
        await telemetry.skill_telemetry(dc.skill_telemetry_ingest_op(GRAPH, extra))

        end = 31 * DAY
        measures = await telemetry.skill_telemetry(
            dc.skill_telemetry_measures_op(GRAPH, skills[0], 0, end)
        )
        ranked = await telemetry.skill_telemetry(
            dc.skill_telemetry_rank_op(GRAPH, 0, end, skills)
        )
        failed = await telemetry.skill_telemetry(
            dc.skill_telemetry_traces_op(GRAPH, skills[0], "fail")
        )
        refused = None
        try:
            await telemetry.skill_telemetry(dc.skill_telemetry_peers_op(GRAPH, 0, end))
        except EngineResponseError as error:
            refused = str(error)
        return {
            "stored": stored,
            "measures": measures,
            "ranked": ranked,
            "failed": failed,
            "refused": refused,
            "events": events,
            "trace": f"{prefix}-trace-1",
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R713
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R713.8")
def test_measures_rank_peers_and_traces(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R713.8
    out = asyncio.run(_journey("tm" + uuid.uuid4().hex[:8]))
    events = out["events"]
    assert out["stored"] == 1000

    # The twelve measures match the figures computed from the same events.
    got = out["measures"]
    assert len(got) == 12
    assert got["invocations"] == 1000
    assert got["distinct_users"] == 30
    agent = sum(1 for event in events if event["chosen_by"] == "agent")
    assert got["agent_chosen_ppm"] == agent * 1_000_000 // 1000
    assert got["user_chosen_ppm"] == (1000 - agent) * 1_000_000 // 1000
    errors = sum(1 for event in events if event["outcome"] != "ok")
    assert got["error_rate_ppm"] == errors * 1_000_000 // 1000
    latencies = sorted(e["ended_ms"] - e["started_ms"] for e in events)
    assert got["latency_p50_ms"] == latencies[499]
    assert got["latency_p95_ms"] == latencies[949]
    assert got["eval_runs"] == 0

    # Three skills rank; none has an evaluation, so busiest first, then by reference.
    order = [row["skill_ref"] for row in out["ranked"]["skills"]]
    assert len(order) == 3 and order[0].endswith("-a@1")

    # Only the failing run of the skill lists as failed.
    assert out["failed"]["trace_ids"] == [out["trace"]]

    # The engine computes the cohort itself: four other principals share the caller's
    # skill class, which is under the floor.
    assert out["refused"] is not None and "PEER_COHORT_TOO_SMALL" in out["refused"]
