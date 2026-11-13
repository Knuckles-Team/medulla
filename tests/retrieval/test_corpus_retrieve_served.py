"""Served corpus retrieval (MEDULLA-FEDERATED-QUERY-R653.3, R653.8 to R653.10).

`CorpusRetrieve` reads the caller's projected graph in one closed mode: grep,
entity neighbourhood, session history or answer lineage.
"""

from __future__ import annotations

import os
import re
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import request_context

_ROOT = Path(__file__).resolve().parents[2]


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _node(client: Any, graph: str, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _edge(
    client: Any, graph: str, src: str, tgt: str, relationship: str, weight: float = 1.0
) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb(
                {"relationship": relationship, "weight": weight}
            ),
        },
        graph,
    )


def _retrieve(client: Any, graph: str, **request: Any) -> dict[str, Any]:
    result = client._send("CorpusRetrieve", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _fixture(client: Any, graph: str) -> None:
    _node(client, graph, "f1", {"type": "File", "body": "alpha\nfn main() {}"})
    _node(client, graph, "f2", {"type": "File", "body": "fn helper() {}"})
    _node(client, graph, "d1", {"type": "Doc", "body": "fn in a doc"})
    for ordinal, text in ((2, "second turn mentions Needle"), (1, "first turn")):
        _node(
            client,
            graph,
            f"turn-{ordinal}",
            {
                "type": "TranscriptTurn",
                "session_id": "sess-1",
                "ordinal": ordinal,
                "role": "user",
                "content": text,
            },
        )
    _node(
        client,
        graph,
        "turn-x",
        {
            "type": "TranscriptTurn",
            "session_id": "sess-2",
            "ordinal": 1,
            "role": "user",
            "content": "needle",
        },
    )
    for node in ("hub", "near-a", "near-b", "far"):
        _node(client, graph, node, {"type": "Entity", "name": node})
    _edge(client, graph, "hub", "near-a", "MENTIONS", 0.9)
    _edge(client, graph, "hub", "near-b", "MENTIONS", 0.4)
    _edge(client, graph, "near-a", "far", "MENTIONS", 0.7)
    for node in ("answer", "src-a", "src-b", "deep"):
        _node(client, graph, node, {"type": "Artifact", "name": node})
    _edge(client, graph, "answer", "src-a", "DERIVED_FROM")
    _edge(client, graph, "answer", "src-b", "CITES")
    _edge(client, graph, "src-a", "deep", "GENERATED_BY")
    _edge(client, graph, "deep", "answer", "CITES")
    _edge(client, graph, "answer", "hub", "RELATED_TO")


def _with_graph(body: Any) -> None:
    graph = f"corpus-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _fixture(client, graph)
        body(client, graph)
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R653.8")
def test_entity_mode(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R653.8: neighbours by weight then id."""

    def body(client: Any, graph: str) -> None:
        rows = _retrieve(client, graph, mode="entity", entity="hub")["entity"]
        assert rows["entity"] == "hub"
        ids = [n["node_id"] for n in rows["neighbours"]]
        assert ids[0] == "near-a" and "near-b" in ids and "far" in ids
        assert "hub" not in ids
        assert all(n["relationship"] for n in rows["neighbours"])
        assert (
            len(
                _retrieve(client, graph, mode="entity", entity="hub", limit=1)[
                    "entity"
                ]["neighbours"]
            )
            == 1
        )

    _with_graph(body)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R653.9")
def test_session_mode(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R653.9: ordinal order, literal filter."""

    def body(client: Any, graph: str) -> None:
        turns = _retrieve(client, graph, mode="session", session_id="sess-1")["session"]
        assert [t["ordinal"] for t in turns] == [1, 2]
        assert all(
            t["session_id"] == "sess-1" and len(t["snippet"]) <= 200 for t in turns
        )
        found = _retrieve(
            client, graph, mode="session", session_id="sess-1", pattern="NEEDLE"
        )["session"]
        assert [t["ordinal"] for t in found] == [2]

    _with_graph(body)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R653.10")
def test_lineage_mode(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R653.10: shortest paths, cycles never repeat."""

    def body(client: Any, graph: str) -> None:
        paths = _retrieve(client, graph, mode="lineage", answer_ref="answer")["lineage"]
        reached = [p["hops"][-1]["node_id"] for p in paths]
        assert reached == ["src-a", "src-b", "deep"], (
            "the cycle to answer and the unrelated edge are skipped"
        )
        assert [h["relationship"] for h in paths[2]["hops"]] == [
            "DERIVED_FROM",
            "GENERATED_BY",
        ]
        assert (
            len(
                _retrieve(client, graph, mode="lineage", answer_ref="answer", limit=2)[
                    "lineage"
                ]
            )
            == 2
        )

    _with_graph(body)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R653.3")
def test_method_reaches_each_mode(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R653.3: served rows match each mode."""

    def body(client: Any, graph: str) -> None:
        matches = _retrieve(
            client, graph, mode="grep", pattern=r"fn \w+", label="File"
        )["matches"]
        assert [(m["node_id"], m["line"]) for m in matches] == [("f1", 2), ("f2", 1)]
        assert matches[0]["start"] == 0 and matches[0]["end"] == 7
        assert all(m["snippet"] for m in matches)
        everything = _retrieve(client, graph, mode="grep", pattern="fn")["matches"]
        assert {m["node_id"] for m in everything} == {"f1", "f2", "d1"}
        with pytest.raises(Exception, match="CORPUS_REGEX_TOO_COMPLEX"):
            _retrieve(client, graph, mode="grep", pattern="(?=a)a")
        with pytest.raises(Exception, match="INVALID_ARGUMENT"):
            _retrieve(client, graph, mode="entity")
        # Nothing is written: a second read answers the same.
        assert (
            _retrieve(client, graph, mode="grep", pattern="fn")["matches"] == everything
        )

    _with_graph(body)
    # One implementation per mode: the handler defines none of its own.
    handler = (
        _ROOT / "crates/medulla-h-query/src/query/corpus_retrieve.rs"
    ).read_text()
    assert "dispatch(" in handler and "expand_hops(" in handler
    assert not re.search(
        r"fn\s+(grep_corpus|entity_context|session_hits|lineage_path)", handler
    )
