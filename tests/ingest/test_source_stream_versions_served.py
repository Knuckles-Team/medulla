"""Served `SourceIngest` stream content versions: MEDULLA-INGEST-WORKER-R910.

Every batch of one connector stream advances one stream-keyed content version.
A second batch admits, a resubmitted batch replays, a racing writer is refused
`STALE_CONTENT_VERSION`, and the receipt names the digest chain.
"""

from __future__ import annotations

import asyncio
import copy
import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
)
from medulla.generated.ingestion import send_source_ingest_status
from medulla.generated.source_ingestion import (
    SourceIngestionDisposition,
    SourceIngestionReceipt,
    SourceIngestStatusRequest,
)

STREAM = "items"
MARKER_PREFIX = "source-live-set:"


def _declaration(connector: str) -> ConnectorPackEntryContent:
    body = {
        "connector": connector,
        "schema_mappings": {
            "item": {
                "fields": {"id": "identifier", "name": "label"},
                "ontology_class": "Document",
            }
        },
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


def _batch(
    connector: str, page: int, previous_page: int | None, name: str
) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": True,
        "records": [
            {
                "stream": STREAM,
                "record_id": "item-1",
                "mapping_reference": f"connector:{connector}#mappings/item",
                "payload": {"id": 1, "name": name},
                "updated_at": "2026-09-20T00:00:00Z",
                "provenance": {
                    "connector": connector,
                    "adapter_kind": "mcp",
                    "server": "demo-server",
                    "tool": "list_items",
                    "tool_schema_sha256": "07" * 32,
                    "source_uri": "demo://items/item-1",
                },
            }
        ],
        "relationships": [],
        "withdrawals": [],
        "provider_checkpoint": {
            "stream": STREAM,
            "position": {"page": page},
            "watermark": "2026-09-20T00:00:00Z",
        },
        "expected_previous_checkpoint": (
            None
            if previous_page is None
            else {"stream": STREAM, "position": {"page": previous_page}}
        ),
    }


async def _submit(
    client: MedullaClient, batch: dict[str, Any], key: str
) -> SourceIngestionReceipt:
    payload = await client._send(
        "SourceIngest",
        {"request": copy.deepcopy(batch)},
        idempotency_key=f"{key}:{uuid.uuid4().hex}",
    )
    return SourceIngestionReceipt.model_validate(payload)


async def _ingest_client(connector: str) -> tuple[MedullaClient, str]:
    """A client on a fresh ingest graph, with the connector's pack imported."""
    admin = await connect()
    graph = f"stream-version-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(admin, connector, [_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


async def _close(client: MedullaClient, graph: str) -> None:
    try:
        await client.tenants.delete(graph)
    finally:
        await client.close()


def _connector() -> str:
    return f"stream-{uuid.uuid4().hex[:10]}"


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.1")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.2")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.3")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.6")
@pytest.mark.asyncio
async def test_second_batch_on_one_stream_admits() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        first = _batch(connector, 2, None, "one")
        second = _batch(connector, 3, 2, "one, revised")
        committed_first = await _submit(client, first, "first")
        committed_second = await _submit(client, second, "second")
        replayed = await _submit(client, second, "second-again")
        assert committed_first.disposition is SourceIngestionDisposition.COMMITTED
        assert committed_second.disposition is SourceIngestionDisposition.COMMITTED
        assert replayed.disposition is SourceIngestionDisposition.REPLAYED
        assert replayed.batch_digest == committed_second.batch_digest
        assert (
            replayed.committed_graph_version == committed_second.committed_graph_version
        )
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.4")
@pytest.mark.asyncio
async def test_racing_writers_one_stale_refusal() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    rival = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        await _submit(client, _batch(connector, 2, None, "one"), "first")
        left = _batch(connector, 3, 2, "left")
        right = _batch(connector, 4, 2, "right")
        outcomes = await asyncio.gather(
            _submit(client, left, "left"),
            _submit(rival, right, "right"),
            return_exceptions=True,
        )
        refusals = [o for o in outcomes if isinstance(o, EngineResponseError)]
        winners = [o for o in outcomes if isinstance(o, SourceIngestionReceipt)]
        assert len(refusals) == 1 and len(winners) == 1, outcomes
        assert "STALE_CONTENT_VERSION" in str(refusals[0])
        status = await send_source_ingest_status(
            client,
            SourceIngestStatusRequest(connector=connector, stream=STREAM),
        )
        assert status.last_batch_digest == winners[0].batch_digest
        assert (
            status.accepted_checkpoint_digest == winners[0].accepted_checkpoint_digest
        )
        # The refused batch left the stream head where the winner put it.
        again = await send_source_ingest_status(
            client,
            SourceIngestStatusRequest(connector=connector, stream=STREAM),
        )
        assert again == status
    finally:
        await rival.close()
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R910.5")
@pytest.mark.asyncio
async def test_receipt_names_digest_chain() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        first = await _submit(client, _batch(connector, 2, None, "one"), "first")
        second_batch = _batch(connector, 3, 2, "one, revised")
        second = await _submit(client, second_batch, "second")
        assert first.previous_content_digest is None
        assert second.previous_content_digest == first.batch_digest
        assert first.stream_object_id == second.stream_object_id
        assert first.stream_object_id.startswith(MARKER_PREFIX)
        assert len(first.stream_object_id) == len(MARKER_PREFIX) + 64
        # A readback of the stored receipt returns the same values.
        readback = await _submit(client, second_batch, "readback")
        assert readback.disposition is SourceIngestionDisposition.REPLAYED
        assert readback.stream_object_id == second.stream_object_id
        assert readback.previous_content_digest == first.batch_digest
        assert readback.receipt_digest == second.receipt_digest
    finally:
        await _close(client, graph)
