"""Served nested schema compatibility of a pack (MEDULLA-TYPED-PACKS-R001.100).

Through the served ``ConnectorPack.Import``: a tool's nested input schema is
compared with the one the active release recorded. Added optional fields commit;
a removed field, a changed type or a changed required status is refused with
``PACK_SCHEMA_INCOMPATIBLE``; a schema naming a base digest the active release
does not hold is refused with ``PACK_PLAN_STALE``. Every refusal leaves the head
untouched, and no import dispatches a tool.
"""

from __future__ import annotations

import asyncio
import copy
import hashlib
import json
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT
from ontology_pack_support import connect, import_entries, violation_codes

from medulla import ConnectorPackClient, ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind, PackHeadRef


def _schema(**extra: Any) -> dict[str, Any]:
    document: dict[str, Any] = {
        "type": "object",
        "properties": {
            "title": {"type": "string"},
            "owner": {
                "type": "object",
                "properties": {"name": {"type": "string"}, "age": {"type": "integer"}},
                "required": ["name"],
            },
        },
        "required": ["title"],
        "x-medulla-schema-version": 1,
    }
    document.update(extra)
    return document


def _bytes(document: dict[str, Any]) -> bytes:
    return json.dumps(document, sort_keys=True).encode()


def _digest(document: dict[str, Any]) -> str:
    return "sha256:" + hashlib.sha256(_bytes(document)).hexdigest()


def _tool(connector: str, document: dict[str, Any]) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/create_ticket",
        name="create_ticket",
        media_type="application/json",
        body=json.dumps(
            {"name": "create_ticket", "description": "Open a ticket."}
        ).encode(),
        input_schema=_bytes(document),
        annotations=PackAnnotations(),
    )


def _head(receipt: dict) -> PackHeadRef:
    return PackHeadRef(
        binding_revision=receipt["binding_revision"],
        pack_digest=receipt["pack_digest"],
    )


async def _status_head(admin: MedullaClient, connector: str) -> tuple[int, str]:
    status = await ConnectorPackClient(admin).status(
        tenant_id=TEST_TENANT, connector=connector
    )
    assert status.head is not None
    return status.head.binding_revision, str(status.head.pack_digest)


def _additive(base: dict[str, Any]) -> dict[str, Any]:
    document = copy.deepcopy(base)
    document["properties"]["labels"] = {"type": "array", "items": {"type": "string"}}
    document["properties"]["owner"]["properties"]["team"] = {"type": "string"}
    document["x-medulla-compat-revision"] = 1
    document["x-medulla-base-digest"] = _digest(base)
    return document


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.100")
def test_nested_schema_evolution_preserves_digest() -> None:
    async def exercise() -> None:
        connector = f"nested-schema-{uuid.uuid4().hex[:8]}"
        first_schema = _schema()
        admin = await connect()
        try:
            first = await import_entries(
                admin, connector, [_tool(connector, first_schema)]
            )
            assert first["result"] == "imported", first
            second_schema = _additive(first_schema)
            second = await import_entries(
                admin,
                connector,
                [_tool(connector, second_schema)],
                expected_head=_head(first["receipt"]),
            )
            assert second["result"] == "imported", second
            assert (await _status_head(admin, connector))[0] == second["receipt"][
                "binding_revision"
            ]
            # The next evolution names the digest the second import recorded.
            third_schema = _additive(second_schema)
            third_schema["properties"]["owner"]["properties"]["email"] = {
                "type": "string"
            }
            third = await import_entries(
                admin,
                connector,
                [_tool(connector, third_schema)],
                expected_head=_head(second["receipt"]),
            )
            assert third["result"] == "imported", third
        finally:
            await admin.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.100")
def test_incompatible_nested_schema_refuses_before_dispatch() -> None:
    async def exercise() -> None:
        connector = f"nested-refuse-{uuid.uuid4().hex[:8]}"
        base = _schema()
        removed = copy.deepcopy(base)
        del removed["properties"]["owner"]["properties"]["age"]
        retyped = copy.deepcopy(base)
        retyped["properties"]["owner"]["properties"]["age"] = {"type": "string"}
        relaxed = copy.deepcopy(base)
        relaxed["required"] = []
        newly_required = copy.deepcopy(base)
        newly_required["properties"]["priority"] = {"type": "string"}
        newly_required["required"] = ["title", "priority"]
        admin = await connect()
        try:
            first = await import_entries(admin, connector, [_tool(connector, base)])
            assert first["result"] == "imported", first
            head = _head(first["receipt"])
            active = await _status_head(admin, connector)
            for broken in (removed, retyped, relaxed, newly_required):
                broken["x-medulla-base-digest"] = _digest(base)
                refused = await import_entries(
                    admin, connector, [_tool(connector, broken)], expected_head=head
                )
                assert violation_codes(refused) == {"PACK_SCHEMA_INCOMPATIBLE"}, refused
                assert await _status_head(admin, connector) == active

            # A base digest the active release does not hold is stale.
            stale = _additive(base)
            stale["x-medulla-base-digest"] = _digest(_schema(extra=True))
            refused = await import_entries(
                admin, connector, [_tool(connector, stale)], expected_head=head
            )
            assert violation_codes(refused) == {"PACK_PLAN_STALE"}, refused
            assert await _status_head(admin, connector) == active

            # An optional nested field the base never named is additive.
            unknown = copy.deepcopy(base)
            unknown["properties"]["owner"]["properties"]["nickname"] = {
                "type": "string"
            }
            accepted = await import_entries(
                admin, connector, [_tool(connector, unknown)], expected_head=head
            )
            assert accepted["result"] == "imported", accepted
        finally:
            await admin.close()

    asyncio.run(exercise())
