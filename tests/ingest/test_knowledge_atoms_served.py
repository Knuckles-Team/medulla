"""Served knowledge atoms: MEDULLA-INGEST-WORKER-R003.3.

A document-class record ingested through `SourceIngest` is decomposed into
provenance-keyed knowledge atoms stored in the batch envelope, each linked to its
document node by an `atom_of` edge. The served `AtomContext` read then selects the
atoms that answer a query and names the provenance key of each.
"""

from __future__ import annotations

import os
import re
import uuid

import pytest
from conftest import tenant_request_context

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_lsh_dedup_served import SOURCE_RECORD_ID, _document_client, _text
from .test_source_stream_versions_served import _batch, _close, _connector, _submit

TEXT = (
    "The ingest worker restarts after every upgrade. "
    "Operators prefer a rolling restart of the worker pool. "
    "Backups run nightly at two in the morning."
)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.3")
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.6")
@pytest.mark.asyncio
async def test_ingest_then_context_cites_provenance() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        batch = _batch(connector, 2, None, "unused")
        record = batch["records"][0]
        record["payload"] = {"id": 1, "text": TEXT}
        batch["records"] = [record]
        receipt = await _submit(client, batch, f"atoms-{uuid.uuid4().hex[:6]}")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED

        document_node = None
        for node_id, _ in await client.nodes.list_by_label("Document", 0):
            props = await client.nodes.properties(node_id)
            if props and props.get(SOURCE_RECORD_ID) == record["record_id"]:
                document_node = node_id
        assert document_node is not None

        context = await client.graph.atom_context("worker restart", 2000)
        assert context["atoms_selected"] == len(context["atoms"]) >= 1
        assert context["tokens_used"] == sum(
            len(atom["text"].split()) for atom in context["atoms"]
        )
        for atom in context["atoms"]:
            # The provenance key names a span of the document it was cut from.
            assert re.fullmatch(r".+#\d+-\d+", atom["provenance_key"])
            # It resolves: the stored atom node links to the document node.
            node = f"knowledge_atom:{atom['atom_id']}"
            edges = await client.edges.properties(node, document_node)
            assert [edge["relationship"] for edge in edges] == ["atom_of"]
            props = await client.nodes.properties(node)
            assert props is not None
            assert props["provenance_key"] == atom["provenance_key"]

        # A query that shares no term selects nothing; a blank query is refused.
        assert (await client.graph.atom_context("zebra"))["atoms"] == []
        with pytest.raises(Exception, match="PARAMETER_INVALID"):
            await client.graph.atom_context("   ")

        # Another tenant reading the same graph selects no atom: refused or empty.
        foreign = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=tenant_request_context("tenant:foreign"),
            graph_name=graph,
        )
        try:
            seen = await foreign.graph.atom_context("worker restart", 2000)
            assert seen["atoms"] == []
        except EngineResponseError:
            pass  # a refused cross-tenant read also sees nothing
        finally:
            await foreign.close()
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.5")
@pytest.mark.asyncio
async def test_counts_in_receipt_and_context() -> None:
    """One original and one near duplicate in one batch: the receipt counts the
    stored atoms and the linked-not-stored atoms, and `AtomContext` reports the
    atoms it selected and their word count."""
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        batch = _batch(connector, 2, None, "unused")
        base = batch["records"][0]
        batch["records"] = [
            {
                **base,
                "record_id": "base",
                "payload": {"id": 1, "text": _text("w", "end")},
            },
            {
                **base,
                "record_id": "near",
                "payload": {"id": 2, "text": _text("w", "tail")},
            },
        ]
        receipt = await _submit(client, batch, f"counts-{uuid.uuid4().hex[:6]}")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        stored = await client.nodes.list_by_label("KnowledgeAtom", 0)
        assert receipt.documents.atoms_emitted == len(stored) >= 1
        assert receipt.documents.atoms_refused >= 1
        assert receipt.documents.duplicates == 1

        context = await client.graph.atom_context("w0", 2000)
        assert context["atoms_selected"] == len(context["atoms"]) >= 1
        assert context["tokens_used"] == sum(
            len(atom["text"].split()) for atom in context["atoms"]
        )
    finally:
        await _close(client, graph)
