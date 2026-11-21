"""Served `SourceIngest` row-version evidence: MEDULLA-INGEST-WORKER-R901.

A record from a table source carries the payload member `_row_version{table, version}`.
The record node stores the opaque `row_version` locus, a newer version supersedes it and
names the replaced evidence, and the receipt counts stored, superseded and refused
versions.
"""

from __future__ import annotations

import re
from typing import Any

import pytest

from .test_source_stream_versions_served import (
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

# The core provenance predicate every ingested record carries
# (MEDULLA-INGEST-WORKER-R003.19), under the one ontology IRI base.
SOURCE_RECORD_ID = "https://knuckles-team.github.io/ontology/core#sourceRecordId"
TABLE_REF = re.compile(r"^eg:source-table:[0-9a-f]{16,128}$")
ROW_REF = re.compile(r"^eg:source-row:[0-9a-f]{16,128}$")


def _versioned(
    connector: str,
    page: int,
    previous: int | None,
    rows: list[tuple[str, Any]],
) -> dict[str, Any]:
    """One batch holding a record per `(record_id, version)` of table `orders`."""
    batch = _batch(connector, page, previous, "unused")
    base = batch["records"][0]
    batch["records"] = [
        {
            **base,
            "record_id": record_id,
            "payload": {
                "id": index,
                "name": record_id,
                "_row_version": {"table": "orders", "version": version},
            },
        }
        for index, (record_id, version) in enumerate(rows)
    ]
    return batch


async def _row_versions(client: Any) -> dict[str, dict[str, Any]]:
    """The properties of every record node, keyed by its source record id."""
    found: dict[str, dict[str, Any]] = {}
    for node_id, _ in await client.nodes.list_by_label("Document", 0):
        props = await client.nodes.properties(node_id)
        if props and props.get(SOURCE_RECORD_ID):
            found[props[SOURCE_RECORD_ID]] = props
    return found


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R901.3")
@pytest.mark.asyncio
async def test_versioned_record_reads_back_its_locus() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        await _submit(client, _versioned(connector, 2, None, [("row-1", 1)]), "v1")
        first = (await _row_versions(client))["row-1"]
        locus = first["row_version"]
        assert locus["version"] == 1
        assert TABLE_REF.match(locus["table_ref"])
        assert ROW_REF.match(locus["row_ref"])
        assert re.fullmatch(r"[0-9a-f]{64}", locus["evidence_digest"])
        assert "row_version_superseded_digest" not in first

        # A newer version supersedes it and names the replaced evidence.
        await _submit(client, _versioned(connector, 3, 2, [("row-1", 2)]), "v2")
        second = (await _row_versions(client))["row-1"]
        assert second["row_version"]["version"] == 2
        assert second["row_version"]["table_ref"] == locus["table_ref"]
        assert second["row_version"]["row_ref"] == locus["row_ref"]
        assert second["row_version"]["evidence_digest"] != locus["evidence_digest"]
        assert second["row_version_superseded_digest"] == locus["evidence_digest"]
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R901.5")
@pytest.mark.asyncio
async def test_receipt_counts_stored_superseded_refused() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        first = await _submit(
            client,
            _versioned(connector, 2, None, [("row-1", 1), ("row-2", "not-a-number")]),
            "stored-and-refused",
        )
        assert first.row_versions.stored == 1
        assert first.row_versions.superseded == 0
        assert first.row_versions.refused == 1
        # The refused record wrote nothing.
        assert first.affected_count == 1
        assert len(first.raw_admissions) == 1
        assert "row-2" not in await _row_versions(client)

        second = await _submit(
            client,
            _versioned(connector, 3, 2, [("row-1", 2)]),
            "superseded",
        )
        assert second.row_versions.stored == 0
        assert second.row_versions.superseded == 1
        assert second.row_versions.refused == 0

        # A version below the stored one is refused.
        third = await _submit(
            client,
            _versioned(connector, 4, 3, [("row-1", 1)]),
            "decreasing",
        )
        assert third.row_versions.refused == 1
        assert third.affected_count == 0
        assert (await _row_versions(client))["row-1"]["row_version"]["version"] == 2
    finally:
        await _close(client, graph)
