"""MEDULLA-DECISION-ENGINE-R174.9.6 served search refresh.

The entry point is the served ``DecisionLog`` method (``learn`` op
``refresh_freshness``) followed by served ``Discover``. The tenant registers a
foreign source named ``search`` whose connector gateway is the loopback
``WebGateway`` replaying the recorded metasearch response. A refresh stores
every result as a ``Document`` node of the tenant graph with its provenance
(source, fetched_at, receipt) and an embedding. A repeated refresh reports every
result unchanged; a changed content digest supersedes the row (the same node,
carrying the previous digest) and marks its region. ``Discover`` returns the
refreshed row.
"""

from __future__ import annotations

import asyncio
import os
import time
from typing import Any

import pytest
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient
from tests.federation.conftest import (  # noqa: F401  (web_gateway is a fixture)
    RECORDED,
    WEB_SERVER,
    WEB_TOOL,
    WebGateway,
    web_gateway,
)

SOURCE = {"Mcp": {"server": WEB_SERVER, "tool": WEB_TOOL}}


def _refresh(graph: str) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "logical_graph": graph,
        "queries": ["needle"],
        "trigger": "staleness",
        "priority": 2,
        "acl_policy_ref": "policy:acl",
        "privacy_policy_ref": "policy:privacy",
        "budget": {"max_requests": 50, "max_cost_units": 500},
    }


def _receipt(recorded: dict[str, Any]) -> dict[str, Any]:
    assert recorded["recorded"] == "refreshed", recorded
    return recorded.get("receipt", recorded)


async def _journey(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)

        async def refresh() -> dict[str, Any]:
            return _receipt(
                await decision.log(
                    dc.learn_op(
                        TEST_TENANT, "refresh_freshness", request=_refresh(graph)
                    )
                )
            )

        first = await refresh()
        assert first["new_count"] == 3, first
        assert len(first["regions"]) == 3, first

        # Discover reads a refreshed row like any other node, with its provenance.
        found = await client.graph.discover(["haystack"], [], k=5)
        hits = [hit for hit in found["hits"] if "Haystack" in (hit["name"] or "")]
        assert hits, found
        node_id = hits[0]["id"]
        props = await client.nodes.properties(node_id)
        assert props["url"] == "https://example.org/haystack", props
        assert props["provenance"]["receipt"] == first["refresh_id"], props
        assert props["provenance"]["fetched_at"] >= 0, props
        assert props["source"], props

        repeated = await refresh()
        assert (repeated["new_count"], repeated["unchanged_count"]) == (0, 3), repeated
        assert repeated["regions"] == [], "a repeat marks no region"

        # A changed content digest supersedes its row and marks its region; the
        # source's two-second cache is crossed first.
        time.sleep(2.5)
        original = RECORDED["rows"][1]["texts"]["snippet"]
        RECORDED["rows"][1]["texts"]["snippet"] = "Where needles were found."
        try:
            changed = await refresh()
        finally:
            RECORDED["rows"][1]["texts"]["snippet"] = original
        assert (changed["superseded_count"], changed["unchanged_count"]) == (1, 2)
        assert len(changed["regions"]) == 1, changed
        again = await client.nodes.properties(node_id)
        assert again["description"] == "Where needles were found.", again
        assert again["supersedes"] == props["content_digest"], again
        assert again["provenance"]["receipt"] == changed["refresh_id"], again
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R174.9.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R174.9.6")
def test_search_refresh_dedupes_and_feeds_refresh(
    clean_graph: Any, web_gateway: WebGateway
) -> None:
    clean_graph.query.register_foreign_source("search", SOURCE)
    asyncio.run(_journey(_graph_name(clean_graph)))
