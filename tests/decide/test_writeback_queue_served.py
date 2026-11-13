"""MEDULLA-DECISION-ENGINE-R615.6 served write-back queue: a high-tier
proposal is queued, approved and released through ``DecisionLog``
``Writeback`` over the wire.

A below-tier proposal and a source conflict refuse at queue time, an early
release refuses ``PROPOSAL_UNAPPROVED``, the first approval is kept on a repeat,
and a release returns one governed change set that a second release repeats.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

AS_OF_MS = 1_700_000_000_000


def _proposal(proposal_id: str, tier: str = "high", **sources: str) -> dict[str, Any]:
    return dc.writeback_proposal(
        proposal_id,
        f"ext:{proposal_id}",
        "sor:ledger",
        tier,
        AS_OF_MS,
        source_values=sources,
    )


async def _refusal(coro: Any) -> str:
    try:
        await coro
    except Exception as error:  # the typed refusal carries the registered code
        return str(error)
    raise AssertionError("the call was expected to refuse")


async def _journey(tag: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)

        async def send(computation: dict[str, Any]) -> Any:
            return await decision.log(dc.writeback_op(TEST_TENANT, computation))

        queue_id = f"q:{tag}"
        first, second = f"wp:{tag}:1", f"wp:{tag}:2"
        below = await _refusal(
            send(dc.writeback_queue(f"{queue_id}:low", [_proposal(first, "low")]))
        )
        conflict = await _refusal(
            send(
                dc.writeback_queue(
                    f"{queue_id}:conflict", [_proposal(first, a="d1", b="d2")]
                )
            )
        )
        queued = await send(
            dc.writeback_queue(queue_id, [_proposal(first), _proposal(second)])
        )
        again = await _refusal(send(dc.writeback_queue(queue_id, [_proposal(first)])))
        early = await _refusal(send(dc.writeback_release(queue_id)))
        approved = await send(dc.writeback_approve(queue_id, first))
        unknown = await _refusal(send(dc.writeback_approve(queue_id, "wp:404")))
        partial = await _refusal(send(dc.writeback_release(queue_id)))
        repeat = await send(dc.writeback_approve(queue_id, first))
        await send(dc.writeback_approve(queue_id, second))
        released = await send(dc.writeback_release(queue_id))
        replay = await send(dc.writeback_release(queue_id))
        late = await _refusal(send(dc.writeback_approve(queue_id, second)))
        missing = await _refusal(send(dc.writeback_release(f"{queue_id}:404")))
        return {
            "below": below,
            "conflict": conflict,
            "queued": queued,
            "again": again,
            "early": early,
            "approved": approved,
            "unknown": unknown,
            "partial": partial,
            "repeat": repeat,
            "released": released,
            "replay": replay,
            "late": late,
            "missing": missing,
            "ids": (first, second),
            "queue_id": queue_id,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R615.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R615.6")
def test_queue_approve_release(clean_graph):
    outcome = asyncio.run(_journey(uuid.uuid4().hex[:12]))
    first, second = outcome["ids"]
    queue_id = outcome["queue_id"]

    assert "WRITEBACK_BELOW_QUEUE_TIER" in outcome["below"]
    assert "WRITEBACK_SOURCE_CONFLICT" in outcome["conflict"]
    assert outcome["queued"]["queue_id"] == queue_id
    assert outcome["queued"]["proposal_ids"] == [first, second]
    assert "WRITEBACK_QUEUE_EXISTS" in outcome["again"]
    assert "PROPOSAL_UNAPPROVED" in outcome["early"]
    assert "WRITEBACK_PROPOSAL_UNKNOWN" in outcome["unknown"]
    assert "PROPOSAL_UNAPPROVED" in outcome["partial"]

    approval = outcome["approved"]["approval"]
    assert approval["proposal_id"] == first
    assert TEST_AGENT_ID in approval["approver"]
    assert approval["approved_at_ms"] > 0
    assert outcome["repeat"]["approval"] == approval

    change_set = outcome["released"]["change_set"]
    assert change_set["change_set_id"] == f"changeset:{queue_id}"
    assert [p["id"] for p in change_set["proposals"]] == [first, second]
    assert len(change_set["approvals"]) == 2
    assert outcome["replay"]["change_set"] == change_set

    assert "WRITEBACK_QUEUE_TRANSITION_REFUSED" in outcome["late"]
    assert "proposal_queue" in outcome["late"] and "released" in outcome["late"]
    assert "WRITEBACK_QUEUE_UNKNOWN" in outcome["missing"]
