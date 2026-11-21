"""One host-driven foreign read, driven to completion through the served lifecycle.

``completed_read`` imports the connector pack of one fixture source, optionally
admits its held mapping through ``ConnectorPackOp::ApproveSource``, opens a bounded
read, pages the fixture's recorded rows with contract provenance and completes it.
It returns the ``execution`` reference a statement binds through
``foreign_executions``. A mapping that is not admitted refuses
``SOURCE_APPROVAL_NOT_ADMITTED`` at open, chunk or complete.
"""

from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from typing import Any

from conftest import TEST_AGENT_ID, TEST_TENANT
from fixture_source import FixtureSource
from test_connector_pack_client import _catalog
from test_connector_pack_planted import _context
from test_contract_kinds_served import (
    ACCESS_PREFIX,
    CLASS_IRI,
    ENTITY,
    KEY_FIELD,
    OPERATION,
    _host_fetch,
    _spec,
)

from medulla import (
    ConnectorPackArchiveBuilder,
    ConnectorPackClient,
    ConnectorPackEntryContent,
)
from medulla.client import MedullaClient
from medulla.connector_pack import connector_pack_import_key
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
    PackProducer,
)


def _contract_iri(connector: str) -> str:
    return f"urn:{connector}-contract#access"


def _contract_ttl(connector: str, kind: str, source_id: str) -> str:
    return (
        f"@prefix ac: <{ACCESS_PREFIX}> .\n"
        f"<{_contract_iri(connector)}> a ac:AccessContract ;\n"
        f' ac:classIri "{CLASS_IRI}" ; ac:sourceId "{source_id}" ;\n'
        f' ac:entity "{ENTITY}" ; ac:keyField "{KEY_FIELD}" ;\n'
        f' ac:accessKind "{kind}" ; ac:operation "{OPERATION}" ;\n'
        f' ac:pagination "none" .'
    )


async def bind_connector(client: MedullaClient, connector: str) -> None:
    """Bind the connector to the importing principal (the first import expects it)."""
    importer = "principal:sha256:" + hashlib.sha256(TEST_AGENT_ID.encode()).hexdigest()
    key = f"host-read:{connector}:bind"
    await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "bind",
                "request": {
                    "context": _context(key),
                    "connector": connector,
                    "importer": importer,
                },
            }
        },
        None,
        idempotency_key=key,
    )


async def completed_read(
    client: MedullaClient,
    *,
    kind: str,
    connector: str,
    source_id: str,
    fixture: FixtureSource,
    plan_digest: str,
    query_digest: str,
    approve: bool = True,
) -> dict[str, Any]:
    """Run one host-driven read of ``fixture`` to completion; return its ``execution``.

    With ``approve=False`` the mapping stays held and the first lifecycle step that
    binds it raises the engine's ``SOURCE_APPROVAL_NOT_ADMITTED`` refusal.
    """
    contract_bytes = _contract_ttl(connector, kind, source_id).encode()
    contract_digest = hashlib.sha256(contract_bytes).hexdigest()
    resource_uri = f"contract://{connector}"

    await client.query.register_foreign_source(source_id, _spec(connector, kind))
    await bind_connector(client, connector)

    server_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.MCP_SERVER,
        uri=f"mcp-server://{connector}",
        name=connector,
        media_type="application/json",
        body=json.dumps({"name": connector}).encode(),
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    tool_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{OPERATION}",
        name=OPERATION,
        media_type="application/json",
        body=b"{}",
        input_schema=b'{"type":"object"}',
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    contract_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri=resource_uri,
        name="source-contract",
        media_type="text/turtle",
        body=contract_bytes,
        input_schema=b'{"type":"object"}',
        output_schema=b'{"type":"object"}',
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    archive = ConnectorPackArchiveBuilder.build(server_entry, [tool_entry, contract_entry])
    blob = await ConnectorPackClient(client).upload_archive(
        archive, operation_key=f"host-read:{connector}:archive"
    )
    index = archive.index(
        connector=connector,
        blob_digest=blob,
        server_package_version="1.0.0",
        producer=PackProducer(name="host-read", version="1"),
        catalog=_catalog(),
    )
    key = connector_pack_import_key(connector, index.pack_digest, None)
    imported = await client._send(
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
        None,
        idempotency_key=key,
    )
    assert imported["result"] == "imported", imported
    receipt = imported["receipt"]
    head = {
        "binding_revision": receipt["binding_revision"],
        "pack_digest": receipt["pack_digest"],
    }

    approval: Any = {
        "batch_id": f"host-read-{connector}-held",
        "committed_version": 1,
        "connector": connector,
        "contract_digest": contract_digest,
        "head": head,
        "registration": {
            "approval": "held",
            "class_iri": CLASS_IRI,
            "connector": connector,
            "contract_iri": _contract_iri(connector),
            "entity": ENTITY,
            "key_field": KEY_FIELD,
            "operation": OPERATION,
            "resource_uri": resource_uri,
            "source_id": source_id,
            "source_kind": kind,
        },
        "schema_version": receipt["schema_version"],
        "tenant_id": TEST_TENANT,
    }
    if approve:
        approve_key = f"host-read:{connector}:approve"
        approval = await client._send(
            "ConnectorPack",
            {
                "op": {
                    "op": "approve_source",
                    "request": {
                        "context": _context(approve_key),
                        "connector": connector,
                        "contract_iri": _contract_iri(connector),
                        "resource_uri": resource_uri,
                        "expected_head": head,
                        "contract_digest": contract_digest,
                    },
                }
            },
            None,
            idempotency_key=approve_key,
        )
        assert approval["registration"]["approval"] == "approved", approval

    opened = await client._send(
        "ForeignReadOpen",
        {
            "request": {
                "schema_version": 1,
                "source_name": source_id,
                "approval": approval,
                "plan_digest": plan_digest,
                "query_digest": query_digest,
                "limits": {
                    "max_rows": 10,
                    "max_bytes": 4096,
                    "chunk_rows": 8,
                    "chunk_bytes": 2048,
                    "deadline_ms": 10_000,
                },
            }
        },
        None,
    )
    execution = opened["execution"]

    status, payload = _host_fetch(fixture, kind)
    assert status == 200, (status, payload)
    items = json.loads(payload)["items"]
    fetch_time = datetime.now(timezone.utc).isoformat()
    rows = [
        {
            "id": item["id"],
            "columns": {"label": item["label"]},
            "provenance": [_contract_iri(connector), f"fetch:{fetch_time}"],
        }
        for item in items
    ]
    chunked = await client._send(
        "ForeignReadChunk",
        {
            "request": {
                "execution": execution,
                "sequence": opened["next_sequence"],
                "source_identity": opened["source_identity"],
                "rows": rows,
            }
        },
        None,
    )
    await client._send(
        "ForeignReadComplete",
        {"request": {"execution": execution, "sequence": chunked["next_sequence"]}},
        None,
    )
    return execution
