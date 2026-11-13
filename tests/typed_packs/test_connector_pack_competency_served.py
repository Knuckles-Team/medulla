"""Served connector-pack competency questions (MEDULLA-CQ-CONN-R084.1 to R084.4).

Each case imports a finance pack or a document pack through the served
``ConnectorPack.Import``, ingests fixture records through the served
``SourceIngest``, then asks the question the fixture directory holds through
served SPARQL. The query, the pack Turtle and the expected rows are the same
files the Rust L2 test (`connector_pack_competency.rs`) reads; ``{c}`` is the
connector name, unique per run. Every answer row carries source, record id and
as-of: the columns after the stable ones must be bound and non-empty.
"""

from __future__ import annotations

import asyncio
import copy
import json
import time
import uuid
from pathlib import Path
from typing import Any

import pytest
from ontology_pack_support import connect, import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

FIXTURES = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "tests"
    / "fixtures"
    / "connector_pack_competency"
)
QUESTIONS = [
    "q1_finance_algorithms",
    "q2_positions_risk",
    "q3_capability_surface",
    "q4_record_governance",
]
STREAM = "items"


def _fixture(*parts: str, connector: str) -> str:
    return FIXTURES.joinpath(*parts).read_text().replace("{c}", connector)


def _entry(
    kind: PackEntryKind, uri: str, name: str, body: bytes, media: str
) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=kind,
        uri=uri,
        name=name,
        media_type=media,
        body=body,
        input_schema=b'{"type":"object"}' if kind is PackEntryKind.TOOL else None,
        annotations=PackAnnotations(),
    )


def _tool(connector: str, name: str) -> ConnectorPackEntryContent:
    body = json.dumps({"name": name, "description": "A tool."}).encode()
    return _entry(
        PackEntryKind.TOOL,
        f"tool://{connector}/{name}",
        name,
        body,
        "application/json",
    )


def _declaration(connector: str, question: str) -> ConnectorPackEntryContent:
    mappings: dict[str, Any] = {
        "position": {
            "fields": {
                "id": "identifier",
                "instrument": "positionInstrument",
                "quantity": "positionQuantity",
            },
            "key_fields": ["id"],
            "ontology_class": "Position",
        },
        "doc": {
            "fields": {"id": "identifier", "title": "label"},
            "key_fields": ["id"],
            "ontology_class": "Document",
        },
    }
    body: dict[str, Any] = {
        "connector": connector,
        "schema_mappings": mappings,
        "sources": [],
    }
    if question == "q4_record_governance":
        body["contract"] = {
            "version": "1.0.0",
            "digest": "0" * 63 + "1",
            "privacy_class": "internal",
            "access": {"default_acl": "quarantine"},
            "provenance": {"required_fields": ["source_uri", "record_id"]},
        }
    return _entry(
        PackEntryKind.RESOURCE,
        "connector://resources",
        "connector-resources",
        json.dumps(body).encode(),
        "application/json",
    )


def _pack(connector: str, question: str) -> list[ConnectorPackEntryContent]:
    if question in ("q1_finance_algorithms", "q2_positions_risk"):
        entries = [
            _tool(connector, name)
            for name in ("var_historical", "stress_portfolio", "price_option")
        ]
        turtle = _fixture("finance_pack.ttl", connector=connector)
    else:
        entries = [
            _tool(connector, "search_documents"),
            _entry(
                PackEntryKind.PROMPT,
                f"prompt://{connector}-doc-prompt",
                f"{connector}-doc-prompt",
                json.dumps({"name": "doc-prompt", "description": "A prompt."}).encode(),
                "application/json",
            ),
            _entry(
                PackEntryKind.SKILL,
                f"skill://{connector}-doc-skill",
                f"{connector}-doc-skill",
                b"---\nname: doc-skill\ndescription: document skill\n---\n\nbody\n",
                "text/markdown",
            ),
        ]
        turtle = _fixture("document_pack.ttl", connector=connector)
    return [
        *entries,
        _declaration(connector, question),
        ontology_entry(connector, "surface", turtle),
    ]


def _record(
    connector: str, name: str, record_id: str, payload: dict[str, Any], at: str
):
    return {
        "stream": STREAM,
        "record_id": record_id,
        "mapping_reference": f"connector:{connector}#mappings/{name}",
        "payload": payload,
        "updated_at": at,
        "provenance": {
            "connector": connector,
            "adapter_kind": "mcp",
            "server": "demo-server",
            "tool": "list_items",
            "tool_schema_sha256": "07" * 32,
            "source_uri": f"demo://items/{record_id}",
        },
    }


def _records(connector: str, question: str) -> list[dict[str, Any]]:
    if question == "q2_positions_risk":
        return [
            _record(
                connector,
                "position",
                "p-1",
                {"id": "p-1", "instrument": "AAPL", "quantity": 10},
                "2026-09-20T00:00:00Z",
            ),
            _record(
                connector,
                "position",
                "p-2",
                {"id": "p-2", "instrument": "MSFT", "quantity": 5},
                "2026-09-21T00:00:00Z",
            ),
        ]
    if question == "q4_record_governance":
        return [
            _record(
                connector,
                "doc",
                "doc-1",
                {"id": "doc-1", "title": "Quarterly report"},
                "2026-09-20T00:00:00Z",
            )
        ]
    return []


async def _ingest(client, connector: str, records: list[dict[str, Any]]) -> None:
    batch = {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": True,
        "records": records,
        "relationships": [],
        "withdrawals": [],
        "provider_checkpoint": {
            "stream": STREAM,
            "position": {"page": 1},
            "watermark": "2026-09-21T00:00:00Z",
        },
        "expected_previous_checkpoint": None,
    }
    await client._send(
        "SourceIngest",
        {"request": copy.deepcopy(batch)},
        idempotency_key=f"pack-competency:{connector}:{uuid.uuid4().hex}",
    )


def _comparable(cell: str, connector: str) -> str:
    """An IRI to its local name, a literal to its lexical form, connector as {c}."""
    if cell.startswith('"'):
        word = cell[1:].split('"', 1)[0]
    else:
        word = cell.strip("<>").replace("#", "/").rsplit("/", 1)[-1]
    return word.replace(connector, "{c}")


def _expected(question: str) -> tuple[int, list[str]]:
    rows = (FIXTURES / question / "expected.tsv").read_text().splitlines()
    return len(rows[0].split("\t")), sorted(rows)


def _columns(query: str) -> list[str]:
    head = query[query.index("SELECT") + len("SELECT") : query.index("WHERE")]
    return [name.lstrip("?") for name in head.split()]


def _requires(question: str, connector: str) -> list[str]:
    """The pack ids the question declares it reads, one per line of its fixture."""
    text = _fixture(question, "requires_packs.txt", connector=connector)
    return [line for line in text.splitlines() if line.strip()]


async def _answer(client, question: str, connector: str) -> list[str]:
    stable, expected = _expected(question)
    query = _fixture(question, "query.rq", connector=connector)
    columns = _columns(query)
    deadline = time.monotonic() + 180
    while True:
        rows = await client.rdf.sparql(
            query, requires_packs=_requires(question, connector)
        )
        for row in rows:
            for column in columns[stable:]:
                assert row.get(column), (column, row)
        found = sorted(
            "\t".join(
                _comparable(row[column], connector) for column in columns[:stable]
            )
            for row in rows
        )
        if found == expected:
            return found
        assert time.monotonic() < deadline, (question, expected, found)
        await asyncio.sleep(0.5)


@pytest.mark.spec("MEDULLA-CQ-CONN-R084.1")
@pytest.mark.spec("MEDULLA-CQ-CONN-R084.2")
@pytest.mark.spec("MEDULLA-CQ-CONN-R084.3")
@pytest.mark.spec("MEDULLA-CQ-CONN-R084.4")
@pytest.mark.parametrize("question", QUESTIONS)
def test_pack_questions_end_to_end(question: str) -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            connector = f"cq-{uuid.uuid4().hex[:8]}"
            imported = await import_entries(
                client, connector, _pack(connector, question)
            )
            assert imported["result"] == "imported", imported
            records = _records(connector, question)
            if records:
                await _ingest(client, connector, records)
            await _answer(client, question, connector)
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-CQ-CONN-R084.7")
@pytest.mark.parametrize("question", QUESTIONS)
def test_pack_question_abstains_when_its_pack_is_not_imported(question: str) -> None:
    """A question whose pack was never imported abstains by name."""

    async def exercise() -> None:
        client = await connect()
        try:
            connector = f"cq-{uuid.uuid4().hex[:8]}"
            query = _fixture(question, "query.rq", connector=connector)
            with pytest.raises(
                EngineResponseError, match="PACK_NOT_IMPORTED"
            ) as refused:
                await client.rdf.sparql(
                    query, requires_packs=_requires(question, connector)
                )
            message = str(refused.value)
            assert "PACK_NOT_IMPORTED" in message, message
            assert connector in message, message
            # Once the pack is imported the same question is no longer refused.
            imported = await import_entries(
                client, connector, _pack(connector, question)
            )
            assert imported["result"] == "imported", imported
            await client.rdf.sparql(
                query, requires_packs=_requires(question, connector)
            )
        finally:
            await client.close()

    asyncio.run(exercise())
