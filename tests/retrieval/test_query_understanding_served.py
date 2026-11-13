"""The one expansion port, served (MEDULLA-FEDERATED-QUERY-R651.3, R651.4, R651.7).

An engine whose `nl_query` section points at a scripted chat endpoint serves `Discover`
mode `deep` and the retrieval round's feedback operator through the one
`ExpansionProvider`. The stub counts every chat call.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from expansion_harness import ChatStub, connect, served_engine

ANSWER = ["a passage about replica recovery", "replica recovery steps", "wal replay"]


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _doc(client: Any, graph: str, node_id: str, name: str) -> None:
    client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb({"type": "Doc", "name": name}),
        },
        graph,
    )


def _deep(client: Any, graph: str, text: str = "how do replicas recover") -> dict:
    return _decode(
        client._send(
            "Discover",
            {
                "keywords": ["replica"],
                "query_embedding": [],
                "k": 5,
                "mode": "deep",
                "query_text": text,
            },
            graph,
        )
    )


def _feedback_round(client: Any, graph: str) -> dict:
    return _decode(
        client._send(
            "RetrievalRound",
            {
                "request": {
                    "plan": {"ops": [{"Scan": {"label": "Doc"}}]},
                    "corpus_revision": "c1",
                    "model_revision": "m1",
                    "feedback": {
                        "feedback": {
                            "original_query": "replica recovery",
                            "round": 1,
                            "corpus_revision": "c1",
                            "model_revision": "m1",
                            "perspectives": [],
                            "documents": [{"document_id": "d1", "relevance": 0.9}],
                            "terms": [
                                {
                                    "term": "wal",
                                    "tier": "reinforce",
                                    "weight": 0.5,
                                    "evidence_document_ids": ["d1"],
                                }
                            ],
                            "limits": {
                                "max_rounds": 4,
                                "max_queries": 3,
                                "max_terms": 8,
                                "max_tokens": 1000,
                                "max_inference_calls": 2,
                            },
                        }
                    },
                }
            },
            graph,
        )
    )


@pytest.fixture(scope="module")
def stub_engine(tmp_path_factory):
    stub = ChatStub(ANSWER)
    with served_engine(tmp_path_factory, stub) as sock:
        yield sock, stub
    stub.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R651.7")
def test_expansion_generation_with_scripted_provider(stub_engine) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R651.7."""
    sock, _ = stub_engine
    client = connect(sock)
    graph = f"gen-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        _doc(client, graph, "d1", "replica notes")
        plan = _deep(client, graph)["plan"]
        assert [e["kind"] for e in plan["expansions"]] == [
            "hypothetical",
            "sub_query",
            "sub_query",
        ]
        assert len({e["digest"] for e in plan["expansions"]}) == 3
    finally:
        client.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R651.3")
def test_one_port_serves_discover_and_feedback(stub_engine) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R651.3."""
    sock, stub = stub_engine
    client = connect(sock)
    graph = f"port-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        _doc(client, graph, "d1", "replica notes")
        before = stub.calls
        _deep(client, graph)
        assert stub.calls == before + 1
        round_out = _feedback_round(client, graph)
        assert stub.calls == before + 2, "the feedback operator asks the same port"
        assert len(round_out["feedback"]["perspectives"]) == 3
    finally:
        client.tenants.delete(graph)
