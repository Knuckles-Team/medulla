"""The client reads the served contract page (MEDULLA-CONTRACT-R054)."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest
from _client_fixtures import PayloadTransport

pytestmark = pytest.mark.no_engine

_DIGEST = "ab" * 32


def _method_row(name: str) -> dict[str, Any]:
    return {
        "id": name,
        "domain": "security",
        "is_wire_callable": True,
        "error_set": ["INVALID_ARGUMENT", "ACCESS_DENIED"],
        "policy": {
            "mutates": False,
            "durability_domain": "None",
            "authz_action": "contract:read",
            "idempotent": True,
            "audited": False,
            "emits_cdc": False,
            "txn_participation": "Snapshot",
        },
        "replay_class": "NotReplayable",
        "consumer_profiles": ["python"],
        "stability": "stable",
        "note": "",
    }


@pytest.mark.spec("MEDULLA-CONTRACT-R054")
def test_client_reads_descriptors_and_scopes() -> None:
    from medulla.client import MedullaClient

    transport = PayloadTransport(
        {
            "DescribeContract": {
                "methods": [
                    _method_row("CheckAccess"),
                    _method_row("DescribeContract"),
                ],
                "scopes": [
                    {
                        "scope": "contract:read",
                        "class": "user",
                        "owner": "engine",
                        "approver_group": None,
                    }
                ],
                "schema_digest": _DIGEST,
                "next_cursor": None,
            }
        }
    )
    client = MedullaClient.__new__(MedullaClient)
    client._send = transport._send  # type: ignore[method-assign]

    async def scenario() -> dict[str, Any]:
        first = await client.describe_contract()
        await client.describe_contract(cursor="DescribeContract", limit=1)
        return first

    page = asyncio.run(scenario())

    # The client sends the contract read with its pagination params.
    assert [call.method for call in transport.sent] == [
        "DescribeContract",
        "DescribeContract",
    ]
    assert transport.sent[0].params == {}
    assert transport.sent[1].params == {"cursor": "DescribeContract", "limit": 1}
    # The page carries the descriptors, the scope registry and the digest.
    assert [row["id"] for row in page["methods"]] == [
        "CheckAccess",
        "DescribeContract",
    ]
    # spec: MEDULLA-CONTRACT-R054
    assert page["methods"][1]["policy"]["authz_action"] == "contract:read"
    # spec: MEDULLA-CONTRACT-R054
    assert page["scopes"][0]["scope"] == "contract:read"
    # spec: MEDULLA-CONTRACT-R054
    assert page["schema_digest"] == _DIGEST
    assert page["next_cursor"] is None
