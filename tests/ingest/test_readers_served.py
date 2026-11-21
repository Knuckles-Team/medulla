"""Served document readers: MEDULLA-INGEST-WORKER-R700.3.

The engine runs the readers inside document normalization. A document-class record
that names a blob (`content_ref`) and a `media_type` is read by the reader of that
media type, and the extracted text takes the same atom path as a `body` record.
"""

from __future__ import annotations

import copy
import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import request_context
from pydantic import TypeAdapter

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries

from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind
from medulla.generated.source_ingestion import (
    QuarantineReason,
    SourceIngestionDisposition,
    SourceIngestionOutcome,
)

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

FIXTURE_DIR = Path(__file__).resolve().parents[2] / "tests/fixtures/readers"
FIXTURES: tuple[tuple[str, str], ...] = (
    ("application/pdf", "sample.pdf"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "sample.docx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "sample.pptx",
    ),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "sample.xlsx"),
    ("message/rfc822", "sample.eml"),
    ("text/html", "sample.html"),
    ("text/csv", "sample.csv"),
    ("text/vtt", "sample.vtt"),
)


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


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R700.4")
@pytest.mark.asyncio
async def test_reader_refusals_quarantine_without_projection() -> None:
    connector = _connector()
    client, graph = await _client(connector)
    try:
        cases = {
            "unsupported": ("application/x-unknown", b"opaque bytes", "READER_UNSUPPORTED_MEDIA_TYPE"),
            "corrupt": ("text/html", b"\xff\xfe\xfd not utf-8", "READER_CORRUPT"),
        }
        documents_before = len(await client.nodes.list_by_label("Document", 0))
        for kind, (media_type, body, rule_id) in cases.items():
            digest = await client.blob.store(body)
            batch = _batch(connector, 2, None, "unused")
            record = batch["records"][0]
            record["record_id"] = f"refused-{kind}"
            record["provenance"]["source_uri"] = f"demo://reader/refused-{kind}"
            record["payload"] = {"id": 1, "blob": f"cas:sha256:{digest}", "media": media_type}
            batch["records"] = [record]
            payload = await client._send(
                "SourceIngest",
                {"request": copy.deepcopy(batch)},
                idempotency_key=f"readers-refusal-{kind}:{uuid.uuid4().hex}",
            )
            outcome = TypeAdapter(SourceIngestionOutcome).validate_python(payload)
            # The file is never skipped: the batch is quarantined under the reader's code.
            assert outcome.outcome == "quarantined", outcome
            assert outcome.receipt.rule_id == rule_id
            assert outcome.receipt.reason is QuarantineReason.NORMALIZATION_FAILED
        # No refusal projected a document.
        assert len(await client.nodes.list_by_label("Document", 0)) == documents_before
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R700.6")
@pytest.mark.asyncio
async def test_each_format_yields_spanned_fragments() -> None:
    """Every committed fixture goes through `BlobPut` and `SourceIngest` and yields a
    document node with at least one linked knowledge atom."""
    connector = _connector()
    client, graph = await _client(connector)
    try:
        batch = _batch(connector, 2, None, "unused")
        template = batch["records"][0]
        records: list[dict[str, Any]] = []
        for index, (media_type, name) in enumerate(FIXTURES):
            digest = await client.blob.store((FIXTURE_DIR / name).read_bytes())
            record = json.loads(json.dumps(template))
            record["record_id"] = f"fixture-{name}"
            record["provenance"]["source_uri"] = f"demo://reader/{name}"
            record["payload"] = {
                "id": index + 1,
                "blob": f"cas:sha256:{digest}",
                "media": media_type,
            }
            records.append(record)
        batch["records"] = records
        receipt = await _submit(client, batch, f"readers-fixtures-{uuid.uuid4().hex[:6]}")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED

        documents: dict[str, str] = {}
        for node_id, _ in await client.nodes.list_by_label("Document", 0):
            props = await client.nodes.properties(node_id) or {}
            record_id = props.get(SOURCE_RECORD_ID, "")
            if record_id.startswith("fixture-"):
                assert record_id not in documents, f"{record_id} has two document nodes"
                documents[record_id] = node_id
                assert props.get("source_raw_sha256")
        assert set(documents) == {f"fixture-{name}" for _, name in FIXTURES}

        atoms = [node for node, _ in await client.nodes.list_by_label("KnowledgeAtom", 0)]
        for record_id, document_node in documents.items():
            linked = 0
            for atom in atoms:
                if await client.edges.properties(atom, document_node):
                    linked += 1
            assert linked >= 1, f"{record_id} yielded no knowledge atom"
    finally:
        await _close(client, graph)
