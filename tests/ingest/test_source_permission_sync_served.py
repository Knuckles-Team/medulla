"""Served source permission sync (MEDULLA-INGEST-WORKER-R902.3, MEDULLA-IDENTITY-R706).

A `groups` batch replaces the member set of one source group without touching any
record. The read gate expands the `group:<id>` references of a record's read roles
through those member sets, so a member removed by a groups batch is denied on the
next read.
"""

from __future__ import annotations

import os
import time

import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

from .test_source_governance_served import _governed, _record
from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

MEMBER = "usr:carol"
OUTSIDER = "usr:dave"
GROUP = "eng"


def _groups_batch(connector: str, page: int, members: list[str]) -> dict:
    return {
        "connector": connector,
        "kind": "groups",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {
            "groups": [
                {
                    "source": connector,
                    "group_id": GROUP,
                    "members": members,
                    "synced_at": int(time.time() * 1000),
                }
            ]
        },
        "provider_checkpoint": {"stream": STREAM, "position": {"permissions": page}},
        "expected_previous_checkpoint": None,
    }


def _reader(principal: str, graph: str) -> SyncMedullaClient:
    context = request_context(agent_id=principal, principal=principal)
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=graph,
    )


def _visible_source_ids(principal: str, graph: str) -> list[str]:
    client = _reader(principal, graph)
    try:
        rows = client.query.uql("MATCH (:Document)").get("rows", [])
        return [row["id"] for row in rows if str(row["id"]).startswith("source:")]
    finally:
        client.close()


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R902.3")
@pytest.mark.spec("MEDULLA-IDENTITY-R706")
@pytest.mark.asyncio
async def test_group_change_applies_without_rewrite() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        records = _governed(
            connector,
            2,
            None,
            [
                _record(
                    connector,
                    "guarded",
                    {
                        "classification": "internal",
                        "access_public": False,
                        "access_read_roles": [f"group:{GROUP}"],
                    },
                )
            ],
        )
        await _submit(client, records, "records")

        # A groups batch writes the member set and nothing else.
        added = await _submit(client, _groups_batch(connector, 1, [MEMBER]), "groups")
        assert added.affected_count == 1
        assert len(added.raw_admissions) == 0
        assert _visible_source_ids(MEMBER, graph), "a group member reads the record"
        assert not _visible_source_ids(OUTSIDER, graph), "a non-member is denied"

        # Deleting the member through a groups batch denies the next read; the
        # record node is never rewritten.
        removed = await _submit(client, _groups_batch(connector, 2, []), "groups")
        assert removed.affected_count == 1
        assert len(removed.raw_admissions) == 0
        assert not _visible_source_ids(MEMBER, graph), "the removed member is denied"
    finally:
        await _close(client, graph)
