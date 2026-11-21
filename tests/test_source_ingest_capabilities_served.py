"""Ingest-capability records served through ``ApplyChangeEnvelope``.

A supersession is two methods every envelope batch already carries
(MEDULLA-REPO-INGEST-R002.651.2): the ``supersedes`` edge from the new fact to the
old one and the ``superseded`` mark on the old fact, committed in the envelope's
one transaction.
"""

from __future__ import annotations

import hashlib
from typing import Any

import msgpack
import pytest
from _harness_envelope import governed_fields
from _harness_envelope import graph_name as _graph_name


def _operation(ordinal: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
    return {
        "ordinal": ordinal,
        "surface": "graph",
        "domain": "graph_rows",
        "method": {"method": method, "params": params},
    }


def _envelope(
    graph: str, envelope_id: str, object_id: str, operations: list[dict[str, Any]]
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "envelope_id": envelope_id,
        "mutation": {
            "batch_id": f"batch:{envelope_id}",
            "graph": graph,
            "idempotency_key": f"supersession:{envelope_id}",
            "operations": operations,
            "outbox": [],
        },
        "content_version": {
            "object_id": object_id,
            "digest_algorithm": "sha256",
            "digest": hashlib.sha256(envelope_id.encode()).hexdigest(),
            "source_version": {"kind": "sequence", "value": 1},
        },
        **governed_fields(object_id),
    }


def _add_node(ordinal: int, node_id: str) -> dict[str, Any]:
    body = {"type": "Fact", "text": node_id}
    return _operation(
        ordinal,
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(body)},
    )


def _edge(ordinal: int, new_fact: str, old_fact: str, envelope_id: str) -> dict:
    # supersession_methods: the edge from the new fact to the old one.
    props = {"relationship": "supersedes", "envelope_id": envelope_id}
    return _operation(
        ordinal,
        "AddEdge",
        {
            "source_id": new_fact,
            "target_id": old_fact,
            "properties_msgpack": msgpack.packb(props),
        },
    )


def _mark(ordinal: int, new_fact: str, old_fact: str, envelope_id: str) -> dict:
    updates = {
        "superseded": True,
        "superseded_by": new_fact,
        "superseded_in": envelope_id,
    }
    return _operation(
        ordinal,
        "CompareAndSetNodeFields",
        {
            "node_id": old_fact,
            "conditions_msgpack": msgpack.packb({}),
            "updates_msgpack": msgpack.packb(updates),
        },
    )


# spec: MEDULLA-REPO-INGEST-R002.651.2
def test_supersession_edge_and_unknown_fact_refusal(clean_graph):
    graph = _graph_name(clean_graph)
    seed = _envelope(
        graph,
        "envelope:facts-seed",
        "fact:old",
        [_add_node(0, "fact:old"), _add_node(1, "fact:new")],
    )
    clean_graph.changes.apply(seed)

    envelope_id = "envelope:supersede-1"
    applied = clean_graph.changes.apply(
        _envelope(
            graph,
            envelope_id,
            "fact:new",
            [
                _edge(0, "fact:new", "fact:old", envelope_id),
                _mark(1, "fact:new", "fact:old", envelope_id),
            ],
        )
    )
    assert applied["envelope_id"] == envelope_id
    edges = [
        (source, target, props)
        for source, target, props in clean_graph.edges.list()
        if (source, target) == ("fact:new", "fact:old")
    ]
    assert len(edges) == 1
    props = edges[0][2]
    if isinstance(props, bytes):
        props = msgpack.unpackb(props)
    assert props["relationship"] == "supersedes"
    assert props["envelope_id"] == envelope_id
    old = clean_graph.nodes.properties("fact:old")
    assert old["superseded"] is True
    assert old["superseded_by"] == "fact:new"
    assert old["superseded_in"] == envelope_id

    # An absent old fact refuses, and so does an edge with no mark; neither writes.
    before = (clean_graph.edges.list(), clean_graph.nodes.properties("fact:new"))
    absent_id = "envelope:supersede-absent"
    with pytest.raises(Exception, match="SUPERSESSION_UNKNOWN_FACT"):
        clean_graph.changes.apply(
            _envelope(
                graph,
                absent_id,
                "fact:new",
                [
                    _edge(0, "fact:new", "fact:absent", absent_id),
                    _mark(1, "fact:new", "fact:absent", absent_id),
                ],
            )
        )
    unpaired_id = "envelope:supersede-unpaired"
    with pytest.raises(Exception, match="SUPERSESSION_UNPAIRED"):
        clean_graph.changes.apply(
            _envelope(
                graph,
                unpaired_id,
                "fact:new",
                [_edge(0, "fact:new", "fact:old", unpaired_id)],
            )
        )
    after = (clean_graph.edges.list(), clean_graph.nodes.properties("fact:new"))
    assert after == before
