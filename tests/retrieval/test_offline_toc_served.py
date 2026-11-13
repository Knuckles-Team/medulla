"""Served source table of contents: MEDULLA-DECISION-ENGINE-R153.6.

A text source ingested through `SourceIngest` (its payload carries a string
`source_text`) is stored with a line-numbered table of contents. `ReadSpan`
then returns one bounded line range, byte-identical to the original text, and
refuses a range past the end of the source.
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

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

STREAM = "items"
SOURCE = "preamble\n# Title\nintro\n\n## Part A\na1\na2\n## Part B\nb1\nb2\nlast line"
LINES = SOURCE.splitlines(keepends=True)


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


def _batch(connector: str) -> dict[str, Any]:
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
                "payload": {"id": 1, "name": "notes", "source_text": SOURCE},
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
            "position": {"page": 1},
            "watermark": "2026-09-20T00:00:00Z",
        },
        "expected_previous_checkpoint": None,
    }


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R153.6")
@pytest.mark.asyncio
async def test_read_span_after_ingest() -> None:
    connector = f"toc-{uuid.uuid4().hex[:10]}"
    graph = f"toc-{uuid.uuid4().hex[:8]}"
    admin = await connect()
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
    try:
        receipt = await client._send(
            "SourceIngest",
            {"request": copy.deepcopy(_batch(connector))},
            idempotency_key=f"toc:{uuid.uuid4().hex}",
        )
        assert receipt["disposition"] == "committed", receipt

        answer = await client.query.uql("MATCH () WHERE type = 'Document'")
        ids = [row["id"] for row in answer["rows"]]
        assert len(ids) == 1, answer
        document_id = ids[0]
        node = await client.nodes.properties(document_id)
        assert node is not None
        revision = node["source_revision"]
        assert node["toc_digest"] and node["source_digest"], node

        for start, end in [(1, 1), (2, 4), (5, 7), (8, 10), (11, 11), (1, len(LINES))]:
            span = await client.graph.read_span(document_id, revision, start, end)
            expected = "".join(LINES[start - 1 : end])
            assert span["text"] == expected, (start, end, span)
            assert span["toc_digest"] == node["toc_digest"], span
            assert span["source_digest"] == node["source_digest"], span
            assert (span["line_start"], span["line_end"]) == (start, end)
            assert (
                SOURCE.encode()[span["byte_start"] : span["byte_end"]]
                == expected.encode()
            )

        with pytest.raises(EngineResponseError, match="SPAN_OUT_OF_BOUNDS"):
            await client.graph.read_span(document_id, revision, 5, len(LINES) + 1)
        with pytest.raises(EngineResponseError, match="DOCUMENT_NOT_FOUND"):
            await client.graph.read_span("no-such-document", revision, 1, 2)
    finally:
        await client.tenants.delete(graph)
        await client.close()
