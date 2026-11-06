"""Shared served-import helpers for the ontology crosswalk and concept path tests.

Builds a pack of ``crosswalk`` / ``concept_path`` JSON entries, imports it
through the hand-written Python client and the real signed import path, and
reads the committed state back through ``OntologyInspect`` and
``GraphSchemaClasses``.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import struct
import time
import uuid
from pathlib import Path
from typing import Any

import msgpack
from conftest import request_context
from test_connector_pack_client import _catalog
from test_connector_pack_planted import IMPORTER, _bind, _context

from medulla import (
    ConnectorPackArchiveBuilder,
    ConnectorPackClient,
    ConnectorPackEntryContent,
)
from medulla.client import (
    EngineResponseError,
    MedullaClient,
    _decode_send_result,
)
from medulla.connector_pack import connector_pack_import_key
from medulla.generated import reasoning
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
    PackProducer,
)

FIXTURE = (
    Path(__file__).resolve().parents[1]
    / "fixtures"
    / "ontology_crosswalk"
    / "core_metadata.json"
)
FIXTURE_SHA256 = "6f8c0ef5d84f5f33910addd861b8b4263343d5f741d2520ceb9f5a80ab5eccc6"


def load_fixture() -> dict[str, Any]:
    raw = FIXTURE.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == FIXTURE_SHA256
    return json.loads(raw)


def crosswalk_digest(record: dict[str, Any]) -> str:
    """The record digest the engine recomputes (``compute_content_digest``)."""

    def field(value: str) -> bytes:
        data = value.encode()
        return struct.pack(">Q", len(data)) + data

    out = b"medulla.vocabulary_crosswalk" + struct.pack(">H", record["schema_version"])
    for name in ("source_class_iri", "target_class_iri", "relation"):
        out += field(record[name])
    for name in ("source_revision", "target_vocabulary_revision", "origin"):
        out += field(record[name])
    targets = record["approved_target_declarations"]
    out += struct.pack(">Q", len(targets))
    for target in targets:
        out += field(target["class_iri"]) + field(target["category"])
    return hashlib.sha256(out).hexdigest()


def crosswalk_record(
    mapping: dict[str, Any], fixture: dict[str, Any], revision: str, **override: Any
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "schema_version": fixture["schema_version"],
        "source_class_iri": mapping["source_class_iri"],
        "target_class_iri": mapping["target_class_iri"],
        "relation": mapping["relation"],
        "source_revision": revision,
        "target_vocabulary_revision": fixture["target_vocabulary_revision"],
        "origin": fixture["origin"],
        "content_digest": "",
        "approved_target_declarations": [
            {
                "class_iri": mapping["target_class_iri"],
                "category": mapping["target_category"],
            }
        ],
    }
    record.update(override)
    record["content_digest"] = crosswalk_digest(record)
    return record


def json_entry(
    kind: PackEntryKind, scheme: str, connector: str, name: str, body: Any
) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=kind,
        uri=f"{scheme}://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps(body, sort_keys=True).encode(),
        annotations=PackAnnotations(),
    )


def crosswalk_entry(
    connector: str, name: str, record: dict[str, Any]
) -> ConnectorPackEntryContent:
    return json_entry(PackEntryKind("crosswalk"), "crosswalk", connector, name, record)


def concept_path_entry(
    connector: str, name: str, record: dict[str, Any]
) -> ConnectorPackEntryContent:
    return json_entry(
        PackEntryKind("concept_path"), "concept-path", connector, name, record
    )


def ontology_entry(connector: str, name: str, turtle: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.ONTOLOGY,
        uri=f"ontology://{connector}/{name}.ttl",
        name=name,
        media_type="text/turtle",
        body=turtle.encode(),
        annotations=PackAnnotations(),
    )


async def connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def core_revision(client: MedullaClient) -> str:
    """The pinned vocabulary revision: the composed digest of the core catalog."""
    view = await reasoning.send_ontology_inspect(
        client, {"documents": [], "source_ids": []}
    )
    assert view.composed_digest
    return view.composed_digest


async def import_entries(
    client: MedullaClient, connector: str, entries: list[ConnectorPackEntryContent]
) -> dict[str, Any]:
    """Import one pack and return the decoded ``ConnectorPack.Import`` result."""
    await _bind(client, connector, IMPORTER)
    server = ConnectorPackEntryContent(
        kind=PackEntryKind.MCP_SERVER,
        uri=f"mcp-server://{connector}",
        name=connector,
        media_type="application/json",
        body=json.dumps({"name": connector}).encode(),
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    packs = ConnectorPackClient(client)
    archive = ConnectorPackArchiveBuilder.build(server, entries)
    blob = await packs.upload_archive(
        archive, operation_key=f"ontology-pack:{connector}:{uuid.uuid4().hex}"
    )
    index = archive.index(
        connector=connector,
        blob_digest=blob,
        server_package_version="1.0.0",
        producer=PackProducer(name="ontology-pack-test", version="1"),
        catalog=_catalog(),
    )
    key = connector_pack_import_key(connector, index.pack_digest, None)
    request = client._build_send_request(
        "ConnectorPack",
        {
            "op": {
                "op": "import",
                "request": {
                    "context": _context(key),
                    "index": index.model_dump(mode="json"),
                    "expected_head": None,
                    "allow_mass_withdrawal": False,
                },
            }
        },
        req_id=client._next_id(),
        target_graph=client._graph_name,
        idempotency_key=key,
    )
    reply = await client._roundtrip(
        msgpack.packb(request, use_bin_type=True),
        req_id=request["id"],
        method="ConnectorPack",
    )
    assert not reply.get("error"), reply
    return _decode_send_result("ConnectorPack", reply.get("result"))


def violation_codes(result: dict[str, Any]) -> set[str]:
    assert result["result"] == "rejected", result
    return {violation["code"] for violation in result["violations"]}


async def attach_pack(client: MedullaClient, connector: str) -> str:
    """Attach the connector's visible pack head; returns the composed digest."""
    deadline = time.monotonic() + 60
    while True:
        try:
            committed = await reasoning.send_graph_schema(
                client,
                {
                    "op": {
                        "op": "attach_pack",
                        "connector": connector,
                        "if_composed_digest": None,
                    }
                },
                idempotency_key=f"attach-pack:{connector}:{uuid.uuid4().hex}",
            )
            return committed.composed_digest
        except EngineResponseError as error:
            if (
                "ATTACH_PACK_HEAD_NOT_VISIBLE" not in str(error)
                or time.monotonic() > deadline
            ):
                raise
            await asyncio.sleep(0.5)


async def inspect_committed(client: MedullaClient) -> Any:
    return await reasoning.send_ontology_inspect(
        client, {"documents": [], "source_ids": []}
    )
