"""MEDULLA-DECISION-ENGINE-R702.3 served human corrections.

L4: every call goes through the running engine over the wire client. A verified
`corrector` records one correction; the engine commits the correction node, its
rule candidate, independent outcome and eval case in one batch, `nearest` finds
it again and `get` reads it back by receipt id. A correction without the role,
a self-reported correction and a read from another tenant are refused.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT, request_context, tenant_request_context

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient

GRAPH = "__commons__"


async def _connect(context: dict[str, object]) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=GRAPH,
    )


async def _seed_action(client: MedullaClient, node_id: str, actor: str) -> None:
    """Store one `ActionRecord` the way ingestion does; a correction targets it."""
    operations = [
        {
            "op": "upsert_node",
            "id": node_id,
            "properties": {
                "type": "ActionRecord",
                "actionKind": "tool_call",
                "actor": actor,
                "idempotencyKey": node_id,
                "tenant_id": TEST_TENANT,
            },
        }
    ]
    await client._send(
        "BatchUpdate",
        {"operations_msgpack": msgpack.packb(operations, use_bin_type=True)},
        GRAPH,
    )


async def _journey(prefix: str) -> dict[str, Any]:
    corrector = await _connect(request_context(roles=["corrector"]))
    plain = await _connect(request_context(roles=["test"]))
    foreign = await _connect(
        tenant_request_context(f"{prefix}-other", roles=["corrector"])
    )
    try:
        decisions = dc.DecisionClient(client=corrector, graph=GRAPH)
        target = f"action:{prefix}"
        own = f"action:{prefix}-own"
        await _seed_action(corrector, target, "agent:planner")
        # An action the corrector itself produced: a self-reported outcome.
        await _seed_action(corrector, own, request_context()["principal"])
        record = dc.correction_record_op(
            correction_id=f"{prefix}-c1",
            target=target,
            before="the wrong answer",
            after="the right answer",
            at_ms=1_700_000_000_000,
        )
        recorded = await decisions.correction_record(record)
        receipt = recorded["receipt_id"]
        nearest = await decisions.correction_record(dc.correction_nearest_op(target))
        read = await decisions.correction_record(dc.correction_get_op(receipt))
        outcomes = await decisions.correction_record(dc.correction_outcomes_op())

        refusals: dict[str, str] = {}
        try:
            await dc.DecisionClient(client=plain, graph=GRAPH).correction_record(record)
        except EngineResponseError as error:
            refusals["role"] = str(error)
        self_reported = dc.correction_record_op(
            correction_id=f"{prefix}-c2",
            target=own,
            before="a",
            after="b",
            at_ms=1,
        )
        try:
            await decisions.correction_record(self_reported)
        except EngineResponseError as error:
            refusals["self"] = str(error)
        try:
            await dc.DecisionClient(client=foreign, graph=GRAPH).correction_record(
                dc.correction_get_op(receipt)
            )
        except EngineResponseError as error:
            refusals["foreign"] = str(error)
        # A target that names no visible action record refuses like an absent one.
        for label, who, target_id in (
            ("absent", decisions, f"action:{prefix}-none"),
            ("foreign_target", dc.DecisionClient(client=foreign, graph=GRAPH), target),
        ):
            try:
                await who.correction_record(
                    dc.correction_record_op(
                        correction_id=f"{prefix}-{label}",
                        target=target_id,
                        before="a",
                        after="b",
                        at_ms=1,
                    )
                )
            except EngineResponseError as error:
                refusals[label] = str(error)
        return {
            "recorded": recorded,
            "nearest": nearest,
            "read": read,
            "outcomes": outcomes,
            "refusals": refusals,
        }
    finally:
        await corrector.close()
        await plain.close()
        await foreign.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R702.3")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R702.4")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R702.5")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R702.6")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R702.8")
def test_correction_served(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R702.3
    # spec: MEDULLA-DECISION-ENGINE-R702.4
    # spec: MEDULLA-DECISION-ENGINE-R702.5
    # spec: MEDULLA-DECISION-ENGINE-R702.6
    # spec: MEDULLA-DECISION-ENGINE-R702.8
    prefix = "corr" + uuid.uuid4().hex[:8]
    result = asyncio.run(_journey(prefix))

    recorded = result["recorded"]
    receipt = recorded["receipt_id"]
    assert len(receipt) == 64
    effect = recorded["effect"]
    assert effect["receipt_id"] == receipt
    assert effect["outcome"]["tag"] == "independent"
    assert effect["outcome"]["value"] == 0.0

    # The recorded correction is its own nearest neighbour.
    assert f"correction:{receipt}" in result["nearest"]["node_ids"]

    read = result["read"]
    assert read["receipt_id"] == receipt
    assert read["correction"]["after"] == "the right answer"
    assert read["effect"] == effect

    # The independent outcome is read back per rule candidate (R702.8).
    candidates = result["outcomes"]["candidates"]
    assert effect["rule_candidate"]["rule_id"] in [c["rule_id"] for c in candidates]
    assert all(c["independent"] >= 1 for c in candidates)

    refusals = result["refusals"]
    assert "CORRECTION_ROLE_REQUIRED" in refusals["role"]
    assert "OUTCOME_SELF_REPORTED" in refusals["self"]
    # The producer is read from the action record, and an invisible target is no oracle.
    assert read["correction"]["produced_by"] == "agent:planner"
    assert read["correction"]["task_class"] == "action_kind:tool_call"
    assert "CORRECTION_NOT_FOUND" in refusals["absent"]
    assert refusals["foreign_target"] == refusals["absent"]
    # A foreign tenant's read is not told the correction exists.
    assert "foreign" in refusals
