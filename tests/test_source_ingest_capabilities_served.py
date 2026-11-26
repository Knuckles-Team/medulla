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


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.651.3")
@pytest.mark.asyncio
async def test_dead_letter_list_and_redrive() -> None:
    import uuid

    from ingest.test_ingest_queue_served import (
        _claim_task,
        _client,
        _with_retry_limits,
    )
    from ingest.test_ingest_queue_served import (
        _envelope as _job_envelope,
    )

    client = await _client()
    try:
        current = await _with_retry_limits(client, 5)
        try:
            receipt = await client._send(
                "IngestEnqueue",
                {"envelope": _job_envelope(f"job-{uuid.uuid4().hex[:10]}")},
            )
            task_id = receipt["task_id"]
            for _ in range(5):
                await _claim_task(client, task_id, "ingest-worker-1")
                await client._send(
                    "TaskQueueAck",
                    {
                        "task_id": task_id,
                        "worker_id": "ingest-worker-1",
                        "outcome": "Failed",
                        "detail": "parse failed",
                    },
                )
            listed = await client._send(
                "DeadLetterList", {"queue": "ingest", "limit": 1000}
            )
            [item] = [i for i in listed["items"] if i["item_id"] == task_id]
            assert item["reason"] == "INGEST_RETRIES_EXHAUSTED"
            assert item["attempts"] == 5
            with pytest.raises(Exception, match="DEAD_LETTER_REQUEST_INVALID"):
                await client._send("DeadLetterList", {"queue": "general", "limit": 10})
            with pytest.raises(Exception, match="DEAD_LETTER_REQUEST_INVALID"):
                await client._send("DeadLetterRedrive", {"item_ids": []})

            redriven = await client._send("DeadLetterRedrive", {"item_ids": [task_id]})
            assert redriven["redriven"] == 1
            again = await client._send(
                "DeadLetterList", {"queue": "ingest", "limit": 1000}
            )
            assert task_id not in [i["item_id"] for i in again["items"]]
            claimed = await _claim_task(client, task_id, "ingest-worker-1")
            assert claimed["attempt"] == 1
            with pytest.raises(Exception, match="DEAD_LETTER_UNKNOWN_ITEM"):
                await client._send("DeadLetterRedrive", {"item_ids": [task_id]})
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.652")
@pytest.mark.asyncio
async def test_each_method_is_served_and_contract_listed() -> None:
    import json

    engine = pytest.importorskip("medulla.engine")
    from ingest.test_ingest_queue_served import _client

    listed = {row["id"] for row in json.loads(engine.method_catalog())["methods"]}
    served = {
        "ApplyChangeEnvelope",
        "SourceIngest",
        "DeadLetterList",
        "DeadLetterRedrive",
        "ProcessConformance",
    }
    assert served <= listed, served - listed
    client = await _client()
    try:
        # Each served ingest-capability read answers over the wire client.
        listed_items = await client._send(
            "DeadLetterList", {"queue": "ingest", "limit": 10}
        )
        assert "items" in listed_items
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.653")
@pytest.mark.asyncio
async def test_refusals_leave_store_digest_unchanged() -> None:
    from ingest.test_ingest_queue_served import _client

    client = await _client()
    try:
        before = await client._send(
            "DeadLetterList", {"queue": "ingest", "limit": 1000}
        )
        with pytest.raises(Exception, match="DEAD_LETTER_UNKNOWN_ITEM"):
            await client._send("DeadLetterRedrive", {"item_ids": ["no-such-task"]})
        with pytest.raises(Exception, match="DEAD_LETTER_REQUEST_INVALID"):
            await client._send("DeadLetterRedrive", {"item_ids": []})
        after = await client._send("DeadLetterList", {"queue": "ingest", "limit": 1000})
        assert after == before
    finally:
        await client.close()


# spec: MEDULLA-REPO-INGEST-R002.653
def test_envelope_without_source_is_refused_by_code(clean_graph):
    graph = _graph_name(clean_graph)
    seed = _envelope(graph, "envelope:no-source", "", [_add_node(0, "fact:nosrc")])
    before = clean_graph.edges.list()
    with pytest.raises(Exception, match="ENVELOPE_SOURCE_MISSING"):
        clean_graph.changes.apply(seed)
    assert clean_graph.edges.list() == before
    assert clean_graph.nodes.properties("fact:nosrc") is None


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.654")
@pytest.mark.asyncio
async def test_evidence_reads_back_under_second_principal() -> None:
    import os

    from conftest import request_context

    from medulla.client import MedullaClient

    second = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="principal:second-reader"),
    )
    try:
        # The dead-letter read answers under another verified principal of the tenant,
        # from the same jobs-store table the first principal's park wrote.
        listed = await second._send(
            "DeadLetterList", {"queue": "ingest", "limit": 1000}
        )
        assert isinstance(listed["items"], list)
        for item in listed["items"]:
            assert item["reason"]
            assert item["attempts"] >= 1
            assert len(item["payload_digest"]) == 64
    finally:
        await second.close()
