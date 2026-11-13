"""Connector-pack generator for the served decision tests.

``make_packs(count, tools)`` builds ``count`` connector packs of ``tools`` tools
each. Every tool declares the capability it provides (``provides``) and a
description from a small deterministic vocabulary, so a job text can be matched
against many packs. The packs are ``(connector, entries)`` pairs ready for
``ontology_pack_support.import_entries``; nothing here talks to the engine.
"""

from __future__ import annotations

import json
import uuid

from medulla import ConnectorPackEntryContent
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

VERBS = ("look up", "create", "update", "close", "list", "export", "label", "sync")
NOUNS = ("order", "invoice", "ticket", "shipment", "customer", "note", "report")


def capability_of(index: int) -> str:
    """The capability the ``index``-th tool of a pack provides."""
    return (
        f"cap:{VERBS[index % len(VERBS)].replace(' ', '-')}-{NOUNS[index % len(NOUNS)]}"
    )


def tool_entry(connector: str, index: int) -> ConnectorPackEntryContent:
    """One tool of ``connector``: name, description and provided capability."""
    name = f"tool{index:04}"
    text = f"{VERBS[index % len(VERBS)]} a {NOUNS[(index // len(VERBS)) % len(NOUNS)]}"
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps({"name": name, "description": text}).encode(),
        input_schema=b'{"properties":{"q":{"type":"string"}},"type":"object"}',
        annotations=PackAnnotations(provides=[capability_of(index)]),
    )


def make_packs(
    count: int, tools: int, *, prefix: str | None = None
) -> list[tuple[str, list[ConnectorPackEntryContent]]]:
    """``count`` packs of ``tools`` tools each, under fresh connector names."""
    stem = prefix or f"gen-{uuid.uuid4().hex[:8]}"
    packs = []
    for number in range(count):
        connector = f"{stem}-{number}"
        packs.append((connector, [tool_entry(connector, i) for i in range(tools)]))
    return packs
