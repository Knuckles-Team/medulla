"""Tag-based masking policies over the served wire (MEDULLA-FEDERATED-QUERY-R706).

Policies are `QueryPolicy` records in the caller's own tenant graph, bound to
columns through promoted classification claims. The loader reads them at plan
time inside the caller's tenant (R706.7); a tenant never sees another tenant's
policy. Two signed clients carry different verified tenants against one engine
(PY-TWO-TENANTS).
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context, tenant_request_context

from medulla.client import MedullaClient

OTHER_TENANT = "tenant:policy-other"


async def _connect(tenant: str) -> MedullaClient:
    context = (
        request_context() if tenant == TEST_TENANT else tenant_request_context(tenant)
    )
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


async def _people(client: MedullaClient, label: str, count: int) -> None:
    for index in range(count):
        await client.nodes.add(
            f"{label}-{index}-{uuid.uuid4().hex}",
            {"type": label, "email": f"person{index}@example.invalid"},
        )


async def _ids(client: MedullaClient, label: str) -> list[str]:
    result = await client.uql(f"MATCH (:{label})")
    return sorted(row["id"] for row in result["rows"])


# spec: MEDULLA-FEDERATED-QUERY-R706.7
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R706.7")
@pytest.mark.asyncio
async def test_cross_tenant_policy_not_applied() -> None:
    """A policy written in tenant A binds only A's reads: tenant B reads the
    same rows it read before the policy existed, and a malformed policy in A
    refuses A's reads with POLICY_RECORD_INVALID while B's reads still run."""
    label = f"PolicyPerson{uuid.uuid4().hex[:8]}"
    tag = f"pii.email.{uuid.uuid4().hex[:8]}"
    tenant_a = await _connect(TEST_TENANT)
    tenant_b = await _connect(OTHER_TENANT)
    try:
        await _people(tenant_a, label, 2)
        await _people(tenant_b, label, 2)
        before_b = await _ids(tenant_b, label)
        assert len(before_b) == 2

        # Tenant A: a promoted claim binds the column to the tag, and an
        # operator-written policy masks the tag.
        await tenant_a.nodes.add(
            f"claim-{uuid.uuid4().hex}",
            {
                "type": "ClassificationClaim",
                "asset": f"col:{label}.email",
                "state": "promoted",
                "categories": [tag],
            },
        )
        await tenant_a.nodes.add(
            f"policy-{uuid.uuid4().hex}",
            {
                "type": "QueryPolicy",
                "kind": "mask",
                "tag": tag,
                "method": "redact",
                "principals_exempt": [],
            },
        )
        assert len(await _ids(tenant_a, label)) == 2, "a valid policy still serves"
        assert await _ids(tenant_b, label) == before_b, "B reads raw rows"

        # A malformed policy in A refuses A's reads; B is untouched.
        await tenant_a.nodes.add(
            f"policy-bad-{uuid.uuid4().hex}",
            {
                "type": "QueryPolicy",
                "kind": "mask",
                "tag": tag,
                "method": "scramble",
            },
        )
        with pytest.raises(Exception) as refused:
            await _ids(tenant_a, label)
        assert "POLICY_RECORD_INVALID" in str(refused.value)
        assert await _ids(tenant_b, label) == before_b
    finally:
        await tenant_a.close()
        await tenant_b.close()
