"""MEDULLA-DECISION-ENGINE-R651.7 served episodic-to-semantic promotion: a
group of episodes reaches the support threshold, the gated `Consolidate` route
takes the current admit receipt, and the semantic node reads back with one
`CONSOLIDATES` edge per episode.

A group below `knob memory.promote_min_support` is refused before the receipt
gate, so it needs no receipt; a group at the threshold still needs one.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any

import msgpack
import pytest
from _memory_receipts import benchmark, pin_policy, submit

from medulla import decision_client as dc
from medulla.client import EngineResponseError

SEMANTIC_ID = "semantic:build"
EPISODES = ["episode:1", "episode:2", "episode:3"]


def _body(payload: Any) -> Any:
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _props() -> dict[str, Any]:
    return {"id": SEMANTIC_ID, "pattern_key": "build", "support_count": 3}


def _candidate_digest(episodes: list[str], props: dict[str, Any]) -> str:
    """The digest the router binds a promotion candidate to: the compact,
    key-sorted JSON of the sorted cluster and the semantic props."""
    canonical = {"episodic_ids": sorted(set(episodes)), "semantic_props": props}
    text = json.dumps(canonical, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode()).hexdigest()


def _consolidate(
    client: Any, episodes: list[str], props: dict[str, Any], receipts: list[str]
) -> Any:
    return client._send(
        "Consolidate",
        {
            "episodic_ids": episodes,
            "semantic_props_msgpack": msgpack.packb(props),
            "admit_receipt_digests": receipts,
        },
    )


# spec: MEDULLA-DECISION-ENGINE-R651.7
def test_episodes_promote_to_semantic_fact(clean_graph):
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

    # Two episodes are below the default threshold of three: refused with the
    # registered code, and without any receipt, because the support check runs
    # ahead of the admit-receipt gate.
    with pytest.raises(EngineResponseError, match="MEMORY_SUPPORT_BELOW_THRESHOLD"):
        _consolidate(client, EPISODES[:2], _props(), [])

    # Three episodes reach the threshold, and the gate then wants a receipt.
    with pytest.raises(EngineResponseError, match="MEMORY_RECEIPT_REQUIRED"):
        _consolidate(client, EPISODES, _props(), [])

    # The receipt binds the semantic node, the cluster digest and the head
    # revision of the semantic node, which is 0 while the node is absent.
    request = benchmark(
        policy_ref,
        "consolidate-1",
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

    semantic = _consolidate(client, EPISODES, _props(), [receipt["receipt_digest"]])
    assert semantic == SEMANTIC_ID

    node = _body(client._send("GetNodeProperties", {"node_id": semantic}))
    assert node["type"] == "SemanticMemory"
    assert node["pattern_key"] == "build"
    assert node["support_count"] == 3
    assert node["consolidated_from_count"] == 3
    assert f"memory:receipt={receipt['receipt_digest']}" in node["premises"]
    for episode in EPISODES:
        edge = _body(
            client._send(
                "GetEdgeProperties", {"source_id": semantic, "target_id": episode}
            )
        )
        assert edge["relationship"] == "CONSOLIDATES", edge
