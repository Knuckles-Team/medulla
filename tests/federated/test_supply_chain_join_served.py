"""Cross-source join over an approved link (MEDULLA-CQ-XSRC-R004.5).

Records of two sources join only across an approved ``owl:sameAs`` edge. The join is
the served UQL traversal over that edge; ``ApproveEntityLink`` is the only writer of
the edge and refuses a pair that is not a match.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

SAME_AS = "`owl:sameAs`"


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _ref(application: str, entity_id: str) -> dict:
    return {"application": application, "entity_id": entity_id}


async def _supplier(client: MedullaClient, ref: dict, name: str, host: str) -> None:
    await client.nodes.add(
        f"{ref['application']}:{ref['entity_id']}",
        {"type": "Supplier", "name": name, "host": host},
    )


async def _linked_ids(client: MedullaClient, start: str) -> set[str]:
    text = f"MATCH (:Supplier) |> WHERE id = '{start}' |> TRAVERSE -[:{SAME_AS}]-"
    result = await client.query.uql(text)
    assert result["kind"] == "rows", result
    return {row["id"] for row in result["rows"]} - {start}


@pytest.mark.spec("MEDULLA-CQ-XSRC-R004.5")
@pytest.mark.asyncio
async def test_join_needs_approved_same_as():
    """No link, then a refused pair, adds no edge; an approved pair joins the record."""
    tag = uuid.uuid4().hex[:8]
    client = await _connect()
    try:
        left = _ref("erpnext", f"SUP-{tag}")
        right = _ref("leanix", f"fs-{tag}")
        other = _ref("leanix", f"fs-other-{tag}")
        await _supplier(client, left, "Acme Corp", "acme.example")
        await _supplier(client, right, "Acme Corp", "acme.example")
        await _supplier(client, other, "Zenith Ltd", "zenith.example")
        start = f"erpnext:SUP-{tag}"

        # No link yet: the traversal reaches no second source.
        assert await _linked_ids(client, start) == set()

        # A pair refused LINK_NOT_APPROVABLE adds no edge; the traversal stays empty.
        with pytest.raises(Exception, match="LINK_NOT_APPROVABLE"):
            await query.send_approve_entity_link(client, {"left": left, "right": other})
        assert await _linked_ids(client, start) == set()

        # After the approval the join returns the second source's record.
        await query.send_approve_entity_link(client, {"left": left, "right": right})
        assert await _linked_ids(client, start) == {f"leanix:fs-{tag}"}
    finally:
        await client.close()
