"""MEDULLA-TYPED-PACKS-R806 served acceptance: schema drift records and repair approval.

A breaking delivery is quarantined (R715). ``SchemaDriftList`` and ``SchemaDriftGet`` read the
quarantine under the caller's tenant. ``SchemaRepairPropose`` derives the repair candidate from
the stored shape and shadow-checks it without installing it. ``SchemaRepairApprove`` activates
it once, only under an approval receipt bound to the candidate digest.
"""

# ruff: noqa: E501, I001

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "source_authority"))
from governed_ledger import (
    candidate_digest,
    consume_approval_lease,
    issue_approval_lease,
)

from medulla.client import EngineResponseError
from medulla.generated import reasoning
from medulla.generated.graph_schema import SchemaSourceOriginViewApproved

# The shared helpers of the R712 drift tests: the approved baseline, the delivery batch and the
# isolated graph.
from test_schema_drift_served import (
    DELIVERY_CLASS,
    DELIVERY_SOURCE_ID,
    DELIVERY_STREAM,
    DRIFTED,
    _approved_delivery_client,
    delivery_batch,
    submit_delivery,
)


def _leases(listed: object) -> list[str]:
    """The approval lease of every source attached under the stream's approved key."""
    return [
        source.origin.approval_lease_id
        for source in listed.dynamic_sources  # type: ignore[attr-defined]
        if source.source_id == DELIVERY_SOURCE_ID
        and isinstance(source.origin, SchemaSourceOriginViewApproved)
    ]


# spec: MEDULLA-TYPED-PACKS-R715
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R806")
@pytest.mark.asyncio
async def test_propose_then_approve_activates_once() -> None:
    connector = f"drift-{uuid.uuid4().hex[:10]}"
    graph = f"drift-methods-{uuid.uuid4().hex[:8]}"
    client = await _approved_delivery_client(connector, graph)
    try:
        # The current data conforms to the drifted candidate, so its shadow check is clean.
        await client.nodes.add("delivery-record", {"type": DELIVERY_CLASS, "name": 7})
        with pytest.raises(EngineResponseError, match="SCHEMA_DRIFT_QUARANTINED"):
            await submit_delivery(
                client, delivery_batch(connector, 3, None, {"name": 7})
            )

        # 1. The quarantine is listed and read back under the caller's tenant.
        listed = await client.schema_drift_list()
        assert len(listed["drifts"]) == 1, listed
        drift = listed["drifts"][0]
        assert drift["connector"] == connector
        assert drift["streams"][0]["stream"] == DELIVERY_STREAM
        assert drift["streams"][0]["report"]["class"] == "breaking"
        assert drift["streams"][0]["report"]["action"] == "quarantined"
        read = await client.schema_drift_get(drift["drift_id"])
        assert read == drift
        assert (await client.schema_drift_list(stream="other-stream"))["drifts"] == []
        with pytest.raises(EngineResponseError, match="DRIFT_NOT_FOUND"):
            await client.schema_drift_get("0" * 64)

        # 2. Propose derives the candidate and applies nothing: the baseline stays in force.
        preview = await client.schema_repair_propose(drift["drift_id"], DELIVERY_STREAM)
        digest = candidate_digest(DRIFTED, DELIVERY_SOURCE_ID)
        assert preview["candidate_digest"] == digest
        assert preview["source_id"] == DELIVERY_SOURCE_ID
        assert preview["applied"] is False
        before = await reasoning.send_graph_schema_list(client, {})
        assert _leases(before) == ["approval:delivery"]
        with pytest.raises(EngineResponseError, match="DRIFT_NOT_FOUND"):
            await client.schema_repair_propose(drift["drift_id"], "other-stream")

        # 3. Each refusal writes nothing.
        with pytest.raises(EngineResponseError, match="REPAIR_APPROVAL_MISSING"):
            await client.schema_repair_approve(
                drift["drift_id"], DELIVERY_STREAM, digest, "approval:absent"
            )
        with pytest.raises(EngineResponseError, match="REPAIR_DIGEST_MISMATCH"):
            await client.schema_repair_approve(
                drift["drift_id"], DELIVERY_STREAM, "0" * 64, "approval:absent"
            )
        lease = "approval:drift-806"
        await issue_approval_lease(client, lease, digest, DELIVERY_SOURCE_ID)
        # A pending approval is not an approval.
        with pytest.raises(EngineResponseError, match="REPAIR_APPROVAL_MISSING"):
            await client.schema_repair_approve(
                drift["drift_id"], DELIVERY_STREAM, digest, lease
            )
        unchanged = await reasoning.send_graph_schema_list(client, {})
        assert unchanged.composed_digest == before.composed_digest

        # 4. The decided approval activates the candidate once.
        await consume_approval_lease(client, lease)
        approved = await client.schema_repair_approve(
            drift["drift_id"], DELIVERY_STREAM, digest, lease
        )
        assert approved["activated"] is True
        assert approved["candidate_digest"] == digest
        after = await reasoning.send_graph_schema_list(client, {})
        assert _leases(after) == [lease]
        again = await client.schema_repair_approve(
            drift["drift_id"], DELIVERY_STREAM, digest, lease
        )
        assert again["activated"] is False
        final = await reasoning.send_graph_schema_list(client, {})
        assert final.composed_digest == after.composed_digest
    finally:
        try:
            await client.tenants.delete(graph)
        finally:
            await client.close()
