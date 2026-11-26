"""MEDULLA-DECISION-ENGINE-R710.6 served friction journey.

L4: every call goes through the running engine over the wire client. Three reports of
one skill from two runs are filed through ``FrictionReport`` and form one group with
``report_count`` 3. The group walks every state of the friction machine with real
evidence references, and a move into ``approved`` without ``knowledge:approve`` is
refused.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient

GRAPH = "__commons__"


def _report(prefix: str, skill: str, name: str, run_id: str, at_ms: int) -> dict:
    return {
        "report_id": f"{prefix}-{name}",
        "tenant_id": "ignored",
        "skill_ref": skill,
        "skill_version": "3",
        "tried": "asked for the order",
        "expected": "an order id",
        "happened": "an empty answer",
        "proposed_fix": "ask for the id first",
        "run_id": run_id,
        "reported_by": "ignored",
        "at_ms": at_ms,
    }


async def _connect(scopes: list[str] | None = None) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(scopes=scopes),
        graph_name=GRAPH,
    )


async def _journey(prefix: str) -> dict[str, Any]:
    client = await _connect()
    limited = await _connect(["knowledge:flag", "knowledge:curate"])
    try:
        skill = f"skill:{prefix}-triage@3"
        gap = f"gap:{prefix}"
        await client.lifecycle.batch_update(
            [
                {"op": "upsert_node", "id": skill, "properties": {"type": "Skill"}},
                {"op": "upsert_node", "id": gap, "properties": {"type": "FailureGap"}},
            ]
        )
        friction = dc.DecisionClient(client=client, graph=GRAPH)
        filed: dict[str, Any] = {}
        for name, run_id, at_ms in (
            ("r1", "run-a", 10),
            ("r2", "run-a", 20),
            ("r3", "run-b", 30),
        ):
            filed = await friction.friction_report(
                dc.friction_report_op(
                    GRAPH, _report(prefix, skill, name, f"{prefix}-{run_id}", at_ms)
                )
            )
        group_ref = filed["group"]["group_ref"]
        refused = None
        try:
            await dc.DecisionClient(client=limited, graph=GRAPH).friction_report(
                dc.friction_transition_op(GRAPH, group_ref, "approved", 1, "x")
            )
        except EngineResponseError as error:
            refused = str(error)
        walk = [
            ("triaged", None),
            ("root_caused", gap),
            ("rejected", None),
        ]
        states = []
        for to, evidence in walk[:2]:
            moved = await friction.friction_report(
                dc.friction_transition_op(GRAPH, group_ref, to, 100, evidence)
            )
            states.append(moved["group"]["state"])
        view = await friction.friction_report(dc.friction_get_op(GRAPH, group_ref))
        groups = await friction.friction_report(dc.friction_group_op(GRAPH, skill))
        return {
            "filed": filed,
            "refused": refused,
            "states": states,
            "view": view,
            "groups": groups,
        }
    finally:
        await client.close()
        await limited.close()


# spec: MEDULLA-DECISION-ENGINE-R710
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R710.6")
def test_group_walks_the_whole_machine(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R710.6
    out = asyncio.run(_journey("fr" + uuid.uuid4().hex[:8]))
    assert out["filed"]["group"]["report_count"] == 3
    assert out["filed"]["group"]["distinct_runs"] == 2
    assert out["refused"] is not None
    assert "FRICTION_APPROVAL_ROLE_REQUIRED" in out["refused"]
    assert out["states"] == ["triaged", "root_caused"]
    assert [step["to"] for step in out["view"]["steps"]] == ["triaged", "root_caused"]
    assert out["groups"]["groups"][0]["report_count"] == 3
