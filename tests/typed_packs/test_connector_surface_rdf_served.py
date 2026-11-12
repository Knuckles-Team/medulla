"""Served connector-surface acceptance (MEDULLA-TYPED-PACKS-R102.7).

Imports a two-tool pack through ``ConnectorPackClient`` and the served
``ConnectorPack.Import``; the projection worker writes one node per tool into
the pack graph, and served SPARQL answers which tools operate on a domain
class. A reimport that withdraws one tool leaves one active tool.
"""

from __future__ import annotations

import asyncio
import json
import time
import uuid

import pytest
from ontology_pack_support import connect, import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
    PackHeadRef,
)

CP = "http://knuckles.team/kg/connector-pack#"


def _tool(connector: str, name: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps({"name": name, "description": "A tool."}).encode(),
        input_schema=b'{"type":"object"}',
        annotations=PackAnnotations(),
    )


def _surface(connector: str, order: str, tools: list[str]) -> ConnectorPackEntryContent:
    turtle = (
        f"@prefix cp: <{CP}> .\n"
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
        f"<{order}> a owl:Class ;\n"
        " rdfs:subClassOf <http://knuckles.team/kg#Agreement> .\n"
    )
    for name in tools:
        turtle += f"<tool://{connector}/{name}> cp:operatesOn <{order}> .\n"
    return ontology_entry(connector, "surface", turtle)


def _pack(connector: str, order: str, tools: list[str]):
    return [_tool(connector, name) for name in tools] + [
        _surface(connector, order, tools)
    ]


async def _active_tools(client, order: str, expected: set[str]) -> set[str]:
    """The component ids of tools on ``order`` once they equal ``expected``."""
    query = (
        f"PREFIX cp: <{CP}> "
        f"SELECT ?id WHERE {{ ?t cp:operatesOn <{order}> ; cp:componentId ?id }}"
    )
    deadline = time.monotonic() + 180
    while True:
        rows = await client.rdf.sparql(query)
        found = {row["id"] for row in rows}
        if found == expected:
            return found
        assert time.monotonic() < deadline, (expected, found)
        await asyncio.sleep(0.5)


async def _component_ids(client, connector: str) -> dict[str, str]:
    rows = await client.rdf.sparql(
        f"PREFIX cp: <{CP}> SELECT ?t ?id WHERE {{ ?t cp:componentId ?id . "
        f'FILTER(STRSTARTS(STR(?t), "tool://{connector}/")) }}'
    )
    return {row["t"].strip("<>"): row["id"] for row in rows}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R102.7")
def test_surface_query_follows_pack_head() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            connector = f"surface-{uuid.uuid4().hex[:8]}"
            order = f"https://example.org/{connector}/Order"
            first = await import_entries(
                client, connector, _pack(connector, order, ["read", "write"])
            )
            assert first["result"] == "imported", first

            await _active_tools(client, order, set(await _both(client, connector)))
            ids = await _component_ids(client, connector)
            assert set(ids) == {f"tool://{connector}/read", f"tool://{connector}/write"}
            assert len(set(ids.values())) == 2

            head = PackHeadRef(
                binding_revision=first["receipt"]["binding_revision"],
                pack_digest=first["receipt"]["pack_digest"],
            )
            second = await import_entries(
                client,
                connector,
                _pack(connector, order, ["read"]),
                expected_head=head,
            )
            assert second["result"] == "imported", second

            kept = ids[f"tool://{connector}/read"]
            assert await _active_tools(client, order, {kept}) == {kept}
        finally:
            await client.close()

    asyncio.run(exercise())


async def _both(client, connector: str) -> list[str]:
    """Wait until both tool nodes are projected; return their component ids."""
    deadline = time.monotonic() + 180
    while True:
        ids = await _component_ids(client, connector)
        if len(ids) == 2:
            return list(ids.values())
        assert time.monotonic() < deadline, ids
        await asyncio.sleep(0.5)
