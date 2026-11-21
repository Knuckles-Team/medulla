"""Served `SourceIngest` as the sole write path for connector batches.

MEDULLA-INGEST-WORKER-R606.3: a canned vendor-extractor batch goes through the
Python client and the stamped nodes read back with their source provenance.
"""

from __future__ import annotations

import pytest

from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    STREAM,
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

KG = "https://knuckles-team.github.io/ontology/core#"


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R606.3")
@pytest.mark.asyncio
async def test_canned_vendor_batch_is_admitted() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        receipt = await _submit(client, _batch(connector, 1, None, "canned"), "canned")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        assert receipt.affected_count == 1

        answer = await client.query.uql(f"MATCH () WHERE source_stream = '{STREAM}'")
        assert answer["kind"] == "rows", answer
        ids = [row["id"] for row in answer["rows"]]
        assert len(ids) == 1, answer
        props = await client.nodes.properties(ids[0])
        assert props is not None, ids
        # The batch writer stamped the record's source identity on the node.
        assert props[KG + "sourceRecordId"] == "item-1", props
        assert props["source_stream"] == STREAM, props
    finally:
        await _close(client, graph)
