"""MEDULLA-DECISION-ENGINE-R801.8 served usage append and price put, and
MEDULLA-DECISION-ENGINE-R801.7 the tenant-scoped usage read.

L4: every call goes through the running engine over the wire client. A service
caller holding ``usage:write`` appends usage records and stores a price
catalog under its verified tenant; a human caller without that scope is
refused; a record naming another tenant is refused with
``USAGE_UNKNOWN_TENANT``; an overlapping price is refused with
``PRICE_WINDOW_OVERLAP``. A caller of a second tenant never sees the first
tenant's ledger: the engine answers only its deployment tenant.
"""

from __future__ import annotations

import asyncio
import os
import time
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

OPERATION = "decide"
TENANT_B = "tenant:usage-b"


async def _usage(op: dict[str, Any], *, scopes: list[str], tenant: str = TEST_TENANT):
    context = request_context(scopes=scopes)
    context["tenant"] = tenant
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )
    try:
        return await dc.DecisionClient(client=client).usage(op)
    finally:
        await client.close()


def _price(valid_from_ms: int, in_price: int) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "model_key": "m1",
        "valid_from_ms": valid_from_ms,
        "in_per_mtok_micro": in_price,
        "out_per_mtok_micro": 15_000_000,
        "cached_per_mtok_micro": 300_000,
    }


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R801.8")
def test_service_append_and_price_put(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R801.8
    child = f"usage-{uuid.uuid4().hex[:8]}"
    now = int(time.time() * 1000)
    records = [
        dc.usage_record(TEST_TENANT, child, OPERATION, f"{child}-run-{i}", now + i)
        for i in range(3)
    ]
    service = ["usage:write"]

    appended = asyncio.run(_usage(dc.usage_append_op(records), scopes=service))
    assert appended["appended"] == 3
    assert len(appended["batch_digest"].removeprefix("sha256:")) == 64

    # A human principal holds no service-only scope.
    with pytest.raises(Exception, match="ACCESS_DENIED"):
        asyncio.run(
            _usage(dc.usage_append_op(records), scopes=["usage:read", "kg:write"])
        )

    # The tenant is the verified caller's: a record naming another refuses.
    foreign = dc.usage_record(TENANT_B, child, OPERATION, f"{child}-b", now)
    with pytest.raises(Exception, match="USAGE_UNKNOWN_TENANT"):
        asyncio.run(_usage(dc.usage_append_op([foreign]), scopes=service))

    # A caller of tenant B is never admitted to tenant A's ledger.
    with pytest.raises(Exception, match="AUTH_TENANT_MISMATCH"):
        asyncio.run(
            _usage(dc.usage_append_op([foreign]), scopes=service, tenant=TENANT_B)
        )

    valid_from = now + 1_000_000 + int(uuid.uuid4().int % 1_000_000)
    stored = asyncio.run(
        _usage(
            dc.usage_put_price_op({"entries": [_price(valid_from, 3_000_000)]}),
            scopes=service,
        )
    )
    assert stored["entries"] == 1
    # An identical replay is an idempotent no-op.
    replay = asyncio.run(
        _usage(
            dc.usage_put_price_op({"entries": [_price(valid_from, 3_000_000)]}),
            scopes=service,
        )
    )
    assert replay["entries"] == 1
    with pytest.raises(Exception, match="PRICE_WINDOW_OVERLAP"):
        asyncio.run(
            _usage(
                dc.usage_put_price_op({"entries": [_price(valid_from - 1, 1)]}),
                scopes=service,
            )
        )


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R801.7")
def test_usage_read_is_tenant_scoped(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R801.7
    child = f"usage-{uuid.uuid4().hex[:8]}"
    base = 4_000_000_000_000 + (uuid.uuid4().int % 1_000_000) * 10_000
    service = ["usage:write"]
    human = ["usage:read", "kg:write"]
    records = [
        dc.usage_record(TEST_TENANT, child, OPERATION, f"{child}-run-{i}", base + i)
        for i in range(1001)
    ]
    appended = asyncio.run(_usage(dc.usage_append_op(records), scopes=service))
    assert appended["appended"] == 1001

    # A human principal with `usage:read` pages the records with a cursor.
    first = asyncio.run(
        _usage(dc.usage_read_op(base, base + 2000, "records"), scopes=human)
    )
    assert first["view"] == "records"
    assert len(first["records"]) == 1000
    assert all(r["tenant"] == TEST_TENANT for r in first["records"])
    assert first["next_cursor"]
    rest = asyncio.run(
        _usage(
            dc.usage_read_op(
                base, base + 2000, "records", cursor=first["next_cursor"]
            ),
            scopes=human,
        )
    )
    assert len(rest["records"]) == 1
    assert rest["next_cursor"] is None

    rollup = asyncio.run(_usage(dc.usage_read_op(base, base + 2000, "rollup"), scopes=human))
    assert rollup["view"] == "rollup"
    assert sum(day["calls"] for day in rollup["days"]) >= 1001

    # A service-only caller without `usage:read` cannot read.
    with pytest.raises(Exception, match="ACCESS_DENIED"):
        asyncio.run(_usage(dc.usage_read_op(base, base + 2000, "records"), scopes=service))

    # A forged cursor of the other tenant refuses before any read.
    forged = f"usage:{TENANT_B}/{0:020d}/{0:010d}".encode().hex()
    with pytest.raises(Exception, match="USAGE_BAD_CURSOR"):
        asyncio.run(
            _usage(
                dc.usage_read_op(base, base + 2000, "records", cursor=forged),
                scopes=human,
            )
        )
    # A caller of tenant B is never admitted to tenant A's ledger.
    with pytest.raises(Exception, match="AUTH_TENANT_MISMATCH"):
        asyncio.run(
            _usage(
                dc.usage_read_op(base, base + 2000, "records"),
                scopes=human,
                tenant=TENANT_B,
            )
        )
