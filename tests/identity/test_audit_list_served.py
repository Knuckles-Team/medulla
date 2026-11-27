"""MEDULLA-IDENTITY-R023.14: the served ``AuditList`` read.

The page lists the caller tenant's operation-audit records ascending by sequence,
keeps one operation on request, resumes after a sequence, and refuses a caller
without the ``audit:operator`` scope.
"""

from __future__ import annotations

import os

import pytest
from conftest import request_context

from medulla.client import MedullaClient


@pytest.mark.spec("MEDULLA-IDENTITY-R023.14")
def test_audit_list_pages_by_sequence_filters_by_op_and_needs_the_operator_scope(
    clean_graph,
):
    import asyncio

    async def scenario() -> None:
        operator = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            timeout=60,
        )
        try:
            page = await operator.admin.audit_list(chain="graph", limit=1)
            assert len(page["records"]) <= 1
            for record in page["records"]:
                assert record["event_line"].startswith("OP_AUDIT|v1|tenant=")
            if page["next_after_seq"] is not None:
                rest = await operator.admin.audit_list(
                    chain="graph", after_seq=page["next_after_seq"]
                )
                assert all(r["seq"] > page["next_after_seq"] for r in rest["records"])
            none = await operator.admin.audit_list(
                chain="graph", op="no.such.operation"
            )
            assert none["records"] == [] and none["next_after_seq"] is None
            with pytest.raises(Exception, match="INVALID_ARGUMENT"):
                await operator.admin.audit_list(chain="graph", limit=501)
        finally:
            await operator.close()
        plain = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(scopes=["graph:read"]),
            timeout=60,
        )
        try:
            with pytest.raises(Exception, match="ACCESS_DENIED|PERMISSION"):
                await plain.admin.audit_list(chain="graph")
        finally:
            await plain.close()

    asyncio.run(scenario())
