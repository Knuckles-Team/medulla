"""MEDULLA-DECISION-ENGINE-R708.6 served knowledge-flag journey.

L4: every call goes through the running engine over the wire client. A recorded run
and a skill node are written to the caller's graph through ``BatchUpdate``.
``KnowledgeFlag`` raises a ``wrong`` flag with a suggested correction, raises the
same flag again and finds one node with ``raise_count`` 2, resolves the flag with
the human correction node and lists no open flags.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

GRAPH = "__commons__"


async def _journey(prefix: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    try:
        run_id = f"{prefix}-run"
        trace = f"{prefix}-trace"
        skill = f"skill:{prefix}-triage@3"
        correction = f"correction:{prefix}"
        await client.lifecycle.batch_update(
            [
                {
                    "op": "upsert_node",
                    "id": trace,
                    "properties": {"node_type": "RunTrace", "run_id": run_id},
                },
                {"op": "upsert_node", "id": skill, "properties": {"type": "Skill"}},
                {
                    "op": "upsert_node",
                    "id": correction,
                    "properties": {"type": "Correction", "correction_id": prefix},
                },
            ]
        )
        decisions = dc.DecisionClient(client=client, graph=GRAPH)

        def raise_op() -> dict[str, Any]:
            return dc.knowledge_flag_raise_op(
                GRAPH,
                "skill",
                "wrong",
                skill,
                "step 3 asks for the wrong field",
                run_id,
                1_700_000_000_000,
                suggested_correction="ask for the order id first",
                trace_node_id=trace,
            )

        first = await decisions.knowledge_flag(raise_op())
        second = await decisions.knowledge_flag(raise_op())
        flag_id = first["flag"]["flag_id"]
        listed = await decisions.knowledge_flag(
            dc.knowledge_flag_list_op(GRAPH, state="open")
        )
        got = await decisions.knowledge_flag(dc.knowledge_flag_get_op(GRAPH, flag_id))
        resolved = await decisions.knowledge_flag(
            dc.knowledge_flag_resolve_op(GRAPH, flag_id, "adopt", correction_id=prefix)
        )
        open_after = await decisions.knowledge_flag(
            dc.knowledge_flag_list_op(GRAPH, state="open")
        )
        return {
            "first": first,
            "second": second,
            "listed": listed,
            "got": got,
            "resolved": resolved,
            "open_after": open_after,
            "flag_id": flag_id,
        }
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R708.6")
def test_raise_dedup_resolve_list(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R708.6
    prefix = "kf" + uuid.uuid4().hex[:8]
    out = asyncio.run(_journey(prefix))

    assert out["first"]["outcome"] == "created"
    assert out["first"]["flag"]["raise_count"] == 1
    # The same flag again keeps one node and counts two raises.
    assert out["second"]["outcome"] == "deduplicated"
    assert out["second"]["flag"]["flag_id"] == out["flag_id"]
    assert out["second"]["flag"]["raise_count"] == 2
    ours = [f for f in out["listed"]["flags"] if f["flag_id"] == out["flag_id"]]
    assert len(ours) == 1 and ours[0]["raise_count"] == 2

    # The flag is agent-asserted and linked to the run trace and its target.
    assert out["got"]["evidence_class"] == "agent_asserted"
    assert out["got"]["observed_in"] == f"{prefix}-trace"
    assert out["got"]["flags"] == f"skill:{prefix}-triage@3"

    # The human correction resolves it; no open flag of ours remains.
    assert out["resolved"]["flag"]["state"] == "curated"
    still_open = [
        f for f in out["open_after"]["flags"] if f["flag_id"] == out["flag_id"]
    ]
    assert still_open == []
