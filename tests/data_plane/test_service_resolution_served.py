"""Service resolution across uptime, container-manager and the CMDB.

MEDULLA-CQ-XSRC-R004.4 and MEDULLA-UNIFIED-DATA-PLANE-R030.3. Three records of one
service stay three nodes with no `owl:sameAs` edge until a principal approves each
link through the served `ApproveEntityLink`. A pair that is not a Match refuses
`LINK_NOT_APPROVABLE`, and a record that does not exist refuses
`LINK_PROPOSAL_UNKNOWN`.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

HOST = "grafana.example.com"


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _ref(application: str, entity_id: str) -> dict:
    return {"application": application, "entity_id": entity_id}


async def _record(
    client: MedullaClient, application: str, entity_id: str, name: str, host: str
) -> None:
    await client.nodes.add(
        f"{application}:{entity_id}",
        {"type": "Service", "name": name, "host": host},
    )


async def _approve(client: MedullaClient, left: dict, right: dict):
    return await query.send_approve_entity_link(client, {"left": left, "right": right})


@pytest.mark.spec("MEDULLA-CQ-XSRC-R004.4")
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R030.3")
@pytest.mark.asyncio
async def test_three_records_resolve_after_approval():
    tag = uuid.uuid4().hex[:8]
    client = await _connect()
    try:
        monitor = _ref("uptime", f"mon-{tag}")
        workload = _ref("container_manager", f"wl-{tag}")
        cmdb = _ref("servicenow", f"ci-{tag}")
        for ref in (monitor, workload, cmdb):
            await _record(client, ref["application"], ref["entity_id"], "Grafana", HOST)
        nodes = [
            f"{r['application']}:{r['entity_id']}" for r in (monitor, workload, cmdb)
        ]

        # Three nodes and no link before any approval.
        assert len(set(nodes)) == 3
        for a, b in ((0, 1), (1, 2), (0, 2)):
            assert not await client.edges.has(nodes[a], nodes[b])

        first = (await _approve(client, monitor, workload)).payload
        second = (await _approve(client, workload, cmdb)).payload
        for approval in (first, second):
            assert approval["predicate"] == "owl:sameAs"
            assert approval["proposal"]["match_class"] == "match"
            assert approval["proposal"]["approved_by"]

        # One equivalence class after approval: the two edges connect all three nodes.
        assert await client.edges.has(nodes[0], nodes[1])
        assert await client.edges.has(nodes[1], nodes[2])
        assert not await client.edges.has(nodes[0], nodes[2])
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-XSRC-R004.4")
@pytest.mark.asyncio
async def test_different_service_and_missing_record_are_refused():
    tag = uuid.uuid4().hex[:8]
    client = await _connect()
    try:
        monitor = _ref("uptime", f"mon-{tag}")
        other = _ref("servicenow", f"ci-{tag}")
        await _record(client, "uptime", monitor["entity_id"], "Grafana", HOST)
        await _record(
            client, "servicenow", other["entity_id"], "Loki", "loki.example.com"
        )
        with pytest.raises(Exception, match="LINK_NOT_APPROVABLE"):
            await _approve(client, monitor, other)
        assert not await client.edges.has(
            f"uptime:{monitor['entity_id']}", f"servicenow:{other['entity_id']}"
        )
        with pytest.raises(Exception, match="LINK_PROPOSAL_UNKNOWN"):
            await _approve(client, monitor, _ref("servicenow", f"absent-{tag}"))
    finally:
        await client.close()
