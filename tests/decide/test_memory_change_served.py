"""MEDULLA-DECISION-ENGINE-R141.103 served proof of the memory-change gate.

The entry points are served ``DecisionEval`` (a memory benchmark that admits) and served
``Consolidate``. Three episodic records are written and the benchmark's receipt digest is named
by ``Consolidate``. A second authorized client reads the new active revision. Repeating
``Consolidate`` without a receipt, and with the now stale one, is refused
``MEMORY_RECEIPT_REQUIRED`` and ``MEMORY_RECEIPT_STALE`` and leaves the node unchanged.
"""

from __future__ import annotations

import os
from typing import Any

import msgpack
import pytest
from _memory_receipts import benchmark, pin_policy, submit
from conftest import request_context
from memory.test_consolidate_served import (
    EPISODES,
    SEMANTIC_ID,
    _body,
    _candidate_digest,
    _consolidate,
    _props,
)

from medulla import decision_client as dc
from medulla.client import EngineResponseError, SyncMedullaClient


def _node(client: Any) -> Any:
    return _body(client._send("GetNodeProperties", {"node_id": SEMANTIC_ID}))


# spec: MEDULLA-DECISION-ENGINE-R141.103
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R141.103")
def test_consolidate_requires_current_admit_receipt(clean_graph):
    client = clean_graph
    policy_ref, _ = pin_policy(client)
    for episode in EPISODES:
        client._send(
            "AddNode",
            {
                "node_id": episode,
                "properties_msgpack": msgpack.packb({"type": "EpisodicMemory"}),
            },
        )
    request = benchmark(
        policy_ref,
        "memory-change-1",
        True,
        record_id=SEMANTIC_ID,
        change_kind="consolidate",
        baseline={"revision": 0, "digest": "0" * 64},
        candidate={"revision": 1, "digest": _candidate_digest(EPISODES, _props())},
    )
    receipt = submit(client, dc.memory_benchmark_op(request))["state"]["output"][
        "receipt"
    ]
    assert receipt["verdict"] == "admit", receipt
    digest = receipt["receipt_digest"]

    assert _consolidate(client, EPISODES, _props(), [digest]) == SEMANTIC_ID

    # A second authorized client reads the new active revision, with the receipt premise.
    reader = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        active = _node(reader)
    finally:
        reader.close()
    assert active["type"] == "SemanticMemory"
    assert active["consolidated_from_count"] == 3
    assert f"memory:receipt={digest}" in active["premises"]

    # The same change without a receipt, and with the receipt now bound to a head revision that
    # has moved, is refused and leaves the node as it was.
    with pytest.raises(EngineResponseError, match="MEMORY_RECEIPT_REQUIRED"):
        _consolidate(client, EPISODES, _props(), [])
    with pytest.raises(EngineResponseError, match="MEMORY_RECEIPT_STALE"):
        _consolidate(client, EPISODES, _props(), [digest])
    assert _node(client) == active
