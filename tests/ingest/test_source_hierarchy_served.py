"""Served `SourceIngest` container hierarchy: ingest half of MEDULLA-INGEST-WORKER-R904.

A `containers` batch carries the folders of one source. The receipt counts the nodes
and `containedIn` edges the batch wrote, a resent tree replays, a parent cycle and a
parent that is neither in the batch nor held are refused, and a record whose mapped
`container_id` names a held container is admitted with its `inContainer` edge.
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
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    STREAM,
    _batch,
    _close,
    _connector,
    _declaration,
    _ingest_client,
    _submit,
)


def _folder_declaration(connector: str) -> ConnectorPackEntryContent:
    """The shared declaration with one more mapped field: `folder` -> `container_id`."""
    declaration = _declaration(connector)
    body = json.loads(declaration.body)
    body["schema_mappings"]["item"]["fields"]["folder"] = "container_id"
    return ConnectorPackEntryContent(
        kind=declaration.kind,
        uri=declaration.uri,
        output_schema=declaration.output_schema,
        name=declaration.name,
        media_type=declaration.media_type,
        body=json.dumps(body).encode(),
        annotations=declaration.annotations,
    )


def _containers_batch(
    connector: str, page: int, previous: int | None, tree: list[tuple[str, str | None]]
) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "containers",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {
            "containers": [
                {
                    "source": connector,
                    "container_id": container_id,
                    "parent_id": parent,
                    "title": container_id,
                }
                for container_id, parent in tree
            ]
        },
        "provider_checkpoint": {"stream": STREAM, "position": {"containers": page}},
        "expected_previous_checkpoint": (
            None
            if previous is None
            else {"stream": STREAM, "position": {"containers": previous}}
        ),
    }


@pytest.mark.asyncio
async def test_container_tree_ingests_and_links_records() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        tree = [("root", None), ("docs", "root"), ("api", "docs")]
        first = await _submit(
            client, _containers_batch(connector, 1, None, tree), "tree"
        )
        assert first.disposition is SourceIngestionDisposition.COMMITTED
        # Three container nodes and two containedIn edges.
        assert first.affected_count == 5
        assert len(first.raw_admissions) == 0
        again = await _submit(
            client, _containers_batch(connector, 1, None, tree), "again"
        )
        assert again.disposition is SourceIngestionDisposition.REPLAYED

        # A parent outside the batch must already be held.
        with pytest.raises(EngineResponseError) as unknown:
            await _submit(
                client,
                _containers_batch(
                    connector, 2, 1, [("lost", f"missing-{uuid.uuid4().hex[:6]}")]
                ),
                "unknown",
            )
        assert "SOURCE_CONTAINER_INVALID" in str(unknown.value)

        # A parent cycle is refused and writes nothing.
        with pytest.raises(EngineResponseError) as cycle:
            await _submit(
                client,
                _containers_batch(connector, 2, 1, [("a", "b"), ("b", "a")]),
                "cycle",
            )
        assert "SOURCE_CONTAINER_CYCLE" in str(cycle.value)
    finally:
        await _close(client, graph)


async def _folder_client(connector: str) -> tuple[MedullaClient, str]:
    """A client on a fresh graph whose connector maps `folder` to `container_id`."""
    admin = await connect()
    graph = f"hierarchy-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(admin, connector, [_folder_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


@pytest.mark.asyncio
async def test_record_in_container_is_admitted_with_its_edge() -> None:
    connector = _connector()
    client, graph = await _folder_client(connector)
    try:
        await _submit(
            client,
            _containers_batch(connector, 1, None, [("root", None), ("docs", "root")]),
            "tree",
        )
        batch = _batch(connector, 2, None, "readme")
        batch["records"][0]["payload"]["folder"] = "docs"
        receipt = await _submit(client, batch, "record")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        assert receipt.affected_count == 1
    finally:
        await _close(client, graph)
