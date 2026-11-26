"""Served `SourceIngest` as the sole write path for connector batches.

MEDULLA-INGEST-WORKER-R606.3: a canned vendor-extractor batch goes through the
Python client and the stamped nodes read back with their source provenance.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError
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


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R606.2")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R606.5")
@pytest.mark.asyncio
async def test_batches_are_stamped_and_routed() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        receipt = await _submit(
            client, _batch(connector, 1, None, "stamped"), "stamped"
        )
        answer = await client.query.uql(f"MATCH () WHERE source_stream = '{STREAM}'")
        node_id = answer["rows"][0]["id"]
        props = await client.nodes.properties(node_id)
        assert props is not None
        assert props["source_system"] == connector, props
        assert props["source_id"] == "item-1", props
        assert props["ingest_time_ms"] > 0, props
        assert props["type"] == "Document", props
        assert receipt.report.written == 1
        assert [c.ontology_class for c in receipt.classes] == ["Document"]
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R606.5")
@pytest.mark.asyncio
async def test_receipt_reports_per_class() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        receipt = await _submit(client, _batch(connector, 1, None, "report"), "report")
        [document] = receipt.classes
        assert document.ontology_class == "Document"
        assert document.report.written == 1
        assert document.report.unchanged == 0
        assert document.report.refused == 0
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R606.4")
@pytest.mark.asyncio
async def test_batch_without_source_system_is_refused_whole() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        batch = _batch(connector, 1, None, "foreign")
        batch["records"][0]["provenance"]["connector"] = f"other-{connector}"
        with pytest.raises(EngineResponseError, match="MATERIALIZE_NO_PROVENANCE"):
            await client._send(
                "SourceIngest", {"request": batch}, idempotency_key="no-provenance"
            )
        answer = await client.query.uql(f"MATCH () WHERE source_stream = '{STREAM}'")
        assert answer["rows"] == [], answer
    finally:
        await _close(client, graph)
