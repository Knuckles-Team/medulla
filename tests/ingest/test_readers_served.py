"""Served document readers: MEDULLA-INGEST-WORKER-R700.3.

The engine runs the readers inside document normalization. A document-class record
that names a blob (`content_ref`) and a `media_type` is read by the reader of that
media type, and the extracted text takes the same atom path as a `body` record.
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

from .test_lsh_dedup_served import SOURCE_RECORD_ID
from .test_source_stream_versions_served import (
    _batch,
    _close,
    _connector,
    _submit,
)

BODIES: dict[str, tuple[str, bytes]] = {
    "csv": ("text/csv", b"name,role\nAda,engineer\nGrace,admiral\n"),
    "html": (
        "text/html",
        b"<html><body><h1>Runbook</h1><p>Restart the worker after every upgrade."
        b"</p></body></html>",
    ),
    "vtt": (
        "text/vtt",
        b"WEBVTT\n\n00:00.000 --> 00:02.000\nWelcome to the review.\n\n"
        b"00:02.000 --> 00:04.000\nThe worker restarts nightly.\n",
    ),
    "srt": (
        "application/x-subrip",
        b"1\n00:00:00,000 --> 00:00:02,000\nWelcome to the review.\n\n"
        b"2\n00:00:02,000 --> 00:00:04,000\nThe worker restarts nightly.\n",
    ),
}


def _declaration(connector: str) -> ConnectorPackEntryContent:
    """A `Document` mapping whose bytes arrive as a blob with a media type."""
    body = {
        "connector": connector,
        "schema_mappings": {
            "item": {
                "fields": {
                    "id": "identifier",
                    "blob": "content_ref",
                    "media": "media_type",
                },
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


async def _client(connector: str) -> tuple[MedullaClient, str]:
    admin = await connect()
    graph = f"readers-{uuid.uuid4().hex[:8]}"
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


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R700.3")
@pytest.mark.asyncio
async def test_text_formats_yield_document_atoms() -> None:
    connector = _connector()
    client, graph = await _client(connector)
    try:
        batch = _batch(connector, 2, None, "unused")
        template = batch["records"][0]
        records: list[dict[str, Any]] = []
        for index, (kind, (media_type, body)) in enumerate(BODIES.items()):
            digest = await client.blob.store(body)
            record = json.loads(json.dumps(template))
            record["record_id"] = f"reader-{kind}"
            record["provenance"]["source_uri"] = f"demo://reader/{kind}"
            record["payload"] = {
                "id": index + 1,
                "blob": f"cas:sha256:{digest}",
                "media": media_type,
            }
            records.append(record)
        batch["records"] = records
        receipt = await _submit(client, batch, f"readers-{uuid.uuid4().hex[:6]}")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED

        documents: dict[str, str] = {}
        for node_id, _ in await client.nodes.list_by_label("Document", 0):
            props = await client.nodes.properties(node_id) or {}
            record_id = props.get(SOURCE_RECORD_ID, "")
            if record_id.startswith("reader-"):
                documents[record_id.removeprefix("reader-")] = node_id
                # The stored document names the digest of the source record.
                assert props.get("source_raw_sha256")
        assert set(documents) == set(BODIES)

        atoms = [node for node, _ in await client.nodes.list_by_label("KnowledgeAtom", 0)]
        for kind, document_node in documents.items():
            linked = 0
            for atom in atoms:
                if await client.edges.properties(atom, document_node):
                    linked += 1
            assert linked >= 1, f"{kind} yielded no knowledge atom"
    finally:
        await _close(client, graph)
