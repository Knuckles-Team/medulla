"""Served `SourceIngest` near-duplicate linking: MEDULLA-INGEST-WORKER-R702.

A document at or above the Jaccard threshold of an earlier original keeps its
record node and is linked to that original by `NEAR_DUPLICATE_OF`. The edge stores
the similarity estimate and the band layout, the receipt counts the duplicates, and
a distinct document is stored as an original.
"""

from __future__ import annotations

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
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    _batch,
    _close,
    _connector,
    _submit,
)

# The core provenance predicate every ingested record carries
# (MEDULLA-INGEST-WORKER-R003.19), under the one ontology IRI base.
SOURCE_RECORD_ID = "https://knuckles-team.github.io/ontology/core#sourceRecordId"
LSH_BANDS = 20
LSH_ROWS = 5
NEAR_DUPLICATES = 10
DISTINCT = 10


def _document_declaration(connector: str) -> ConnectorPackEntryContent:
    """A `Document` mapping whose `text` member is the document body."""
    body = {
        "connector": connector,
        "schema_mappings": {
            "item": {
                "fields": {"id": "identifier", "text": "body"},
                "ontology_class": "Document",
            }
        },
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        output_schema=b'{"type":"object"}',
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


async def _document_client(connector: str) -> tuple[MedullaClient, str]:
    admin = await connect()
    graph = f"lsh-dedup-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(admin, connector, [_document_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


def _text(prefix: str, last: str) -> str:
    words = [f"{prefix}{index}" for index in range(59)] + [last]
    return " ".join(words) + "."


def _documents(connector: str) -> list[dict[str, Any]]:
    """One base document, ten near duplicates of it and ten distinct documents."""
    base = _batch(connector, 1, None, "unused")["records"][0]
    texts = [("base", _text("w", "end"))]
    texts += [(f"near-{n}", _text("w", f"variant{n}")) for n in range(NEAR_DUPLICATES)]
    texts += [(f"other-{n}", _text(f"d{n}x", "end")) for n in range(DISTINCT)]
    return [
        {**base, "record_id": record_id, "payload": {"id": index, "text": text}}
        for index, (record_id, text) in enumerate(texts)
    ]


async def _ingest(
    client: MedullaClient, connector: str, key: str
) -> tuple[Any, dict[str, str]]:
    batch = _batch(connector, 2, None, "unused")
    batch["records"] = _documents(connector)
    receipt = await _submit(client, batch, key)
    ids: dict[str, str] = {}
    for node_id, _ in await client.nodes.list_by_label("Document", 0):
        props = await client.nodes.properties(node_id)
        if props and props.get(SOURCE_RECORD_ID):
            ids[props[SOURCE_RECORD_ID]] = node_id
    return receipt, ids


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R702.4")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R702.6")
@pytest.mark.asyncio
async def test_near_duplicates_link_and_distinct_pass() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        receipt, ids = await _ingest(client, connector, "dedup")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        assert receipt.documents.duplicates == NEAR_DUPLICATES
        # A near duplicate keeps its record node: every document has one.
        assert len(ids) == 1 + NEAR_DUPLICATES + DISTINCT
        base = ids["base"]
        for n in range(NEAR_DUPLICATES):
            edges = await client.edges.properties(ids[f"near-{n}"], base)
            assert [edge["relationship"] for edge in edges] == ["NEAR_DUPLICATE_OF"]
        # A distinct document links to nothing and carries its signature.
        for n in range(DISTINCT):
            other = ids[f"other-{n}"]
            assert not await client.edges.has(other, base)
            props = await client.nodes.properties(other)
            assert props is not None
            assert len(props["minhash_signature"]) == LSH_BANDS * LSH_ROWS
            assert props["minhash_first_version"] > 0
        # A near duplicate is never indexed: it carries no signature.
        near = await client.nodes.properties(ids["near-0"])
        assert near is not None
        assert "minhash_signature" not in near
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R702.5")
@pytest.mark.asyncio
async def test_link_stores_similarity_estimate() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        _, ids = await _ingest(client, connector, "similarity")
        edges = await client.edges.properties(ids["near-3"], ids["base"])
        assert len(edges) == 1
        edge = edges[0]
        assert 0.8 <= edge["similarity_estimate"] <= 1.0
        assert edge["lsh_bands"] == LSH_BANDS
        assert edge["lsh_rows"] == LSH_ROWS
    finally:
        await _close(client, graph)
