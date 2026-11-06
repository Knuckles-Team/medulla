"""Served ontology edit chain (MEDULLA-TYPED-PACKS-R651.3 to R651.6).

``GraphSchema`` ``ImportOntology`` appends one edit per revision to a hash
chain stored as ``OntologyEdit`` graph nodes; ``RevertEdit`` appends the
inversions newest first; a governed (approved) attach seals its edit; naming
findings are ``NamingFinding`` nodes. Each test uses its own connector and
ontology IRI, so the edits of one never touch another's subject.
"""

from __future__ import annotations

import asyncio
import os
import sys
import uuid
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import request_context
from ontology_pack_support import connect

from medulla.client import EngineResponseError, MedullaClient

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "packs"))
from test_schema_drift_served import (
    BASELINE,
    BASELINE_RECORD,
    DRIFTED,
    DRIFTED_RECORD,
    SOURCE_ID,
    approved_attach,
    close_isolated_graph,
    connect_isolated_graph,
)

HEADER = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
EDIT_LABEL = "OntologyEdit"
FINDING_LABEL = "NamingFinding"


def unique_iri() -> str:
    return f"http://example.org/edits/{uuid.uuid4().hex}"


def class_body(iri: str, *names: str) -> str:
    return HEADER + "".join(f"<{iri}#{name}> a owl:Class .\n" for name in names)


def decode(props: Any) -> dict[str, Any]:
    return msgpack.unpackb(props, raw=False) if isinstance(props, bytes) else props


async def node_props(client: MedullaClient, label: str) -> list[dict[str, Any]]:
    rows = await client.nodes.list_by_label(label)
    return [decode(props) for _, props in sorted(rows, key=lambda row: row[0])]


async def edits_of(client: MedullaClient, subject: str) -> list[dict[str, Any]]:
    return [
        e for e in await node_props(client, EDIT_LABEL) if e["subject_iri"] == subject
    ]


async def import_body(
    client: MedullaClient, connector: str, iri: str, body: str
) -> dict:
    return await client.rdf.import_ontology(
        connector, iri, "1", body, idempotency_key=f"import:{uuid.uuid4().hex}"
    )


async def with_client(
    exercise: Callable[[MedullaClient], Awaitable[None]],
) -> None:
    client = await connect()
    try:
        await exercise(client)
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R651.3")
def test_schema_ops_append_edits_and_revert_edit_inverts() -> None:
    async def exercise(client: MedullaClient) -> None:
        iri = unique_iri()
        await import_body(client, "edits-a", iri, class_body(iri, "Alpha"))
        await import_body(client, "edits-a", iri, class_body(iri, "Alpha", "Beta"))
        edits = await edits_of(client, iri)
        assert [e["kind"] for e in edits] == ["add", "change"]
        assert edits[1]["prev_digest"] is not None
        await client.rdf.revert_edit(iri, idempotency_key=f"revert:{uuid.uuid4().hex}")
        after = await edits_of(client, iri)
        assert [e["kind"] for e in after] == ["add", "change", "change", "remove"]
        assert after[2]["before_digest"] == edits[1]["after_digest"]
        assert after[3]["after_digest"] is None

    asyncio.run(with_client(exercise))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R651.4")
def test_broken_chain_sealed_revert_and_naming_refuse() -> None:
    async def exercise(client: MedullaClient) -> None:
        # A governed attach seals its edit; reverting across it refuses. The
        # attach runs on a graph holding only its conforming record, which the
        # shadow ingest re-validates first.
        governed, graph = await connect_isolated_graph()
        try:
            await approved_attach(governed, "approval:edits", BASELINE, BASELINE_RECORD)
            with pytest.raises(EngineResponseError, match="EDIT_SEALED"):
                await governed.rdf.revert_edit(SOURCE_ID)
        finally:
            await close_isolated_graph(governed, graph)
        # Naming: a snake_case class is a finding (warn) or refused (refuse).
        iri = unique_iri()
        bad = class_body(iri, "bad_name")
        if os.environ.get("MEDULLA_ONTOLOGY_NAMING_MODE") == "1":
            with pytest.raises(EngineResponseError, match="NAMING_VIOLATION"):
                await import_body(client, "edits-b", iri, bad)
            assert await edits_of(client, iri) == []
        else:
            await import_body(client, "edits-b", iri, bad)
        # A tampered edit node breaks the chain; the next write refuses.
        nodes = await client.nodes.list_by_label(EDIT_LABEL)
        newest_id = sorted(row[0] for row in nodes)[-1]
        original = dict(decode(dict(nodes)[newest_id]))
        await client.nodes.add(
            newest_id, {**original, "subject_iri": "http://example.org/x"}
        )
        try:
            with pytest.raises(EngineResponseError, match="EDIT_CHAIN_BROKEN"):
                await import_body(
                    client, "edits-c", unique_iri(), class_body(iri, "Gamma")
                )
        finally:
            await client.nodes.add(newest_id, original)

    asyncio.run(with_client(exercise))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R651.5")
def test_head_inversions_and_findings_read_back() -> None:
    async def exercise(client: MedullaClient) -> None:
        iri = unique_iri()
        first = await import_body(
            client, "edits-d", iri, class_body(iri, "Alpha", "bad_name")
        )
        edits = await edits_of(client, iri)
        assert first["edit_head"] is not None
        reverted = await client.rdf.revert_edit(
            iri, idempotency_key=f"r:{uuid.uuid4().hex}"
        )
        assert reverted["edit_head"] != first["edit_head"]
        assert [e["kind"] for e in await edits_of(client, iri)] == [
            edits[0]["kind"],
            "remove",
        ]
        findings = await node_props(client, FINDING_LABEL)
        assert any(f["subject_iri"].endswith("#bad_name") for f in findings)

    asyncio.run(with_client(exercise))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R651.6")
def test_import_edit_chain_revert_and_sealed_refusal() -> None:
    async def exercise(client: MedullaClient) -> None:
        iri = unique_iri()
        await import_body(client, "edits-e", iri, class_body(iri, "Alpha"))
        await import_body(client, "edits-e", iri, class_body(iri, "Alpha", "Beta"))
        await client.rdf.revert_edit(iri, idempotency_key=f"r:{uuid.uuid4().hex}")
        chain = await edits_of(client, iri)
        assert len(chain) == 4
        # A second tenant keeps its own chain, in a graph of its own: it sees
        # none of the first tenant's edits and its import starts a new chain.
        context = request_context()
        context["tenant"] = "tenant-edits-b"
        other_graph = f"edits-b-{uuid.uuid4().hex[:8]}"
        other = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=context,
            graph_name=other_graph,
        )
        try:
            await other.tenants.create(other_graph)
            assert await edits_of(other, iri) == []
            await import_body(other, "edits-e", iri, class_body(iri, "Alpha"))
            own = await edits_of(other, iri)
            assert [e["kind"] for e in own] == ["add"]
            assert own[0]["prev_digest"] is None
            await other.tenants.delete(other_graph)
        finally:
            await other.close()
        # A sealed edit refuses a revert.
        governed, graph = await connect_isolated_graph()
        try:
            await approved_attach(governed, "approval:edits-6", DRIFTED, DRIFTED_RECORD)
            with pytest.raises(EngineResponseError, match="EDIT_SEALED"):
                await governed.rdf.revert_edit(SOURCE_ID)
        finally:
            await close_isolated_graph(governed, graph)

    asyncio.run(with_client(exercise))
