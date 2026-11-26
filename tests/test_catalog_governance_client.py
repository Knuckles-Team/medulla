"""CatalogRead governance and proposals client vectors (R108.18).

Static client checks against a fake transport; no engine. The generated sender
accepts the two read ops, refuses a field the contract does not name, and the
decoders return the roles and curation rows exactly as the engine answered them.
"""

from __future__ import annotations

import asyncio

import pytest
from _client_fixtures import PayloadTransport
from pydantic import ValidationError

from medulla.generated import query

pytestmark = pytest.mark.no_engine

GOVERNANCE = {
    "kind_counts": [],
    "rows": [],
    "governance": {"owners": ["o1"], "stewards": ["s1", "s2"]},
}
PROPOSALS = {
    "kind_counts": [],
    "rows": [],
    "next_cursor": "2",
    "proposals": [
        {
            "id": "description_proposal:t:term:abc",
            "kind": "description",
            "target": "term:revenue",
            "state": "proposed",
            "opened_by": "agent-1",
            "text": "Recognised revenue.",
            "change_request": "cr-1",
        },
        {
            "id": "cr-1",
            "kind": "change_request",
            "target": "term:revenue",
            "state": "open",
            "opened_by": "agent-1",
            "reviewers": ["s1"],
        },
    ],
}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.18")
def test_governance_read_sends_the_target_and_decodes_the_roles() -> None:
    transport = PayloadTransport({"CatalogRead": GOVERNANCE})
    params = {"op": {"op": "governance", "target": {"asset": "table:orders"}}}
    result = asyncio.run(query.send_catalog_read(transport, params, graph="tenant"))
    assert transport.sent[0].params == params
    answer = query.decode_catalog_read_governance(result)
    assert answer.governance is not None
    assert answer.governance.owners == ["o1"]
    assert answer.governance.stewards == ["s1", "s2"]
    assert answer.proposals == []


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.18")
def test_proposals_read_decodes_both_kinds_and_refuses_unknown_fields() -> None:
    transport = PayloadTransport({"CatalogRead": PROPOSALS})
    params = {"op": {"op": "proposals", "state": "open", "limit": 2}}
    result = asyncio.run(query.send_catalog_read(transport, params, graph="tenant"))
    answer = query.decode_catalog_read_proposals(result)
    assert [row.kind.value for row in answer.proposals] == [
        "description",
        "change_request",
    ]
    assert answer.proposals[1].reviewers == ["s1"]
    assert answer.next_cursor == "2"
    with pytest.raises(ValidationError):
        asyncio.run(
            query.send_catalog_read(
                transport,
                {"op": {"op": "proposals", "owner": "someone"}},
                graph="tenant",
            )
        )
    assert len(transport.sent) == 1
