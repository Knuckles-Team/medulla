"""Served `DocumentDelete`: wiring, refusals and the deletion record.

MEDULLA-UNIFIED-DATA-PLANE-R651.3, R651.4 and R651.5 over the session engine
started by the `start_medulla_server` fixture of `tests/conftest.py`.
"""

from __future__ import annotations

import os

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import EngineResponseError, SyncMedullaClient

DOC = "doc:delete-proof"


def _build(client: SyncMedullaClient, chunks: int = 3, doc: str = DOC) -> list[str]:
    client.nodes.add(doc, {"type": "Document", "text": "body"})
    ids = [doc]
    for ordinal in range(chunks):
        chunk = f"{doc}:chunk:{ordinal}"
        fact = f"{doc}:fact:{ordinal}"
        client.nodes.add(chunk, {"type": "DocumentChunk", "ordinal": ordinal})
        client.edges.add(chunk, doc, {"relationship": "chunkOf"})
        client.nodes.add(fact, {"type": "ExtractedFact"})
        client.edges.add(fact, chunk, {"relationship": "extractedFrom"})
        ids += [chunk, fact]
    return ids


def _delete(client: SyncMedullaClient, doc: str = DOC, **extra) -> dict:
    request = {"tenant": TEST_TENANT, "document_id": doc, **extra}
    return client._send("DocumentDelete", {"request": request})


def _counts(client: SyncMedullaClient) -> tuple[int, int]:
    return client.nodes.count(), client._send("EdgeCount")


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R651.3")
def test_method_registered_and_scoped(clean_graph):
    _build(clean_graph)
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    no_scope = SyncMedullaClient.connect(
        socket_path=socket_path,
        verified_context=request_context(scopes=["graph:read"]),
    )
    try:
        with pytest.raises(EngineResponseError):
            _delete(no_scope)
    finally:
        no_scope.close()
    with pytest.raises(EngineResponseError, match="DOCUMENT_NOT_FOUND"):
        clean_graph._send(
            "DocumentDelete",
            {"request": {"tenant": "tenant:other", "document_id": DOC}},
        )
    assert clean_graph.nodes.has(DOC)
    receipt = _delete(clean_graph)
    assert receipt["tenant"] == TEST_TENANT
    assert not clean_graph.nodes.has(DOC)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R651.4")
def test_delete_refusals_change_nothing(clean_graph):
    _build(clean_graph)
    clean_graph.nodes.add("note:outside", {"type": "Note"})
    clean_graph.edges.add("note:outside", f"{DOC}:chunk:0", {"relationship": "cites"})
    before = _counts(clean_graph)
    with pytest.raises(EngineResponseError, match="DOCUMENT_NOT_FOUND"):
        _delete(clean_graph, doc="doc:absent")
    with pytest.raises(EngineResponseError, match="DOCUMENT_HAS_REFERENCES"):
        _delete(clean_graph)
    assert _counts(clean_graph) == before
    clean_graph.edges.remove("note:outside", f"{DOC}:chunk:0")
    before = _counts(clean_graph)
    with pytest.raises(EngineResponseError, match="CASCADE_TOO_LARGE"):
        _delete(clean_graph, max_nodes=2)
    assert _counts(clean_graph) == before


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R651.5")
def test_deletion_record_readable(clean_graph):
    ids = _build(clean_graph)
    receipt = _delete(clean_graph)
    assert receipt["nodes_deleted"] == len(ids)
    assert receipt["edges_deleted"] == 2 * 3
    record_id = f"document-deletion:{receipt['plan_digest']}"
    record = clean_graph.nodes.properties(record_id)
    assert record["type"] == "DocumentDeletion"
    assert record["document_id"] == DOC
    for field in ("plan_digest", "nodes_deleted", "edges_deleted"):
        assert record[field] == receipt[field]
    assert record["actor"]
    assert record["deleted_at"] > 0
    assert "text" not in record
