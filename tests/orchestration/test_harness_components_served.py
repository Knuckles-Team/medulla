"""MEDULLA-ORCH-VOCAB-R801.6 served receipt: two versions admitted and readable.

The entry point is the generated client write through
``ApplyChangeEnvelopes`` (``changes.apply_batch`` compiles to it): one
envelope publishes version v1, the next publishes v2. Both are admitted and
read back with no window close and no ``supersedes_id``; currency is the
champion pointer move of R801.7.
"""

from __future__ import annotations

from typing import Any

import msgpack
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op

from medulla.client import EngineResponseError
from medulla.decision import digest_text

COMPONENT = "comp:served-prompt"
CONTENT_DIGEST_DOMAIN = "eg/harness-component-content/v1"


def content_digest(content_ref: str) -> str:
    """The vocabulary's content digest — the engine's one digest scheme."""
    return digest_text(CONTENT_DIGEST_DOMAIN, content_ref)


def version_node_id(content_ref: str) -> str:
    return f"harness-component:{COMPONENT}:{content_digest(content_ref)}"


def version_props(
    content_ref: str,
    valid_from: int,
    tx_from: int,
    confidence: float = 0.8,
) -> dict[str, Any]:
    """One published version as the writer composes it: the record's fields
    plus the engine-owned write marker and the client's transaction stamp."""
    props: dict[str, Any] = {
        "_harness_component": True,
        "component_id": COMPONENT,
        "kind": "system_prompt",
        "content_ref": content_ref,
        "content_digest": content_digest(content_ref),
        "valid_from": valid_from,
        "confidence": confidence,
        "evidence_run_id": f"run:{COMPONENT}:{valid_from}",
        "applied_to_id": "agent:served",
        "tx_from": tx_from,
    }
    return props


# spec: MEDULLA-ORCH-VOCAB-R801.6
def test_two_versions_are_admitted_and_readable(clean_graph):
    graph = _graph_name(clean_graph)
    v1_id = version_node_id("blob:v1")
    v2_id = version_node_id("blob:v2")

    # Two versions of one component, each its own envelope: no supersedes
    # pointer, no edge and no window-close CAS. Currency is the champion
    # pointer move of R801.7, not a write-time supersession.
    first = _envelope(
        graph,
        v1_id,
        "h2-r801:v1",
        1,
        [
            op(
                0,
                "AddNode",
                {
                    "node_id": v1_id,
                    "properties_msgpack": msgpack.packb(
                        version_props("blob:v1", valid_from=10, tx_from=100)
                    ),
                },
            )
        ],
    )
    second = _envelope(
        graph,
        v2_id,
        "h2-r801:v2",
        2,
        [
            op(
                0,
                "AddNode",
                {
                    "node_id": v2_id,
                    "properties_msgpack": msgpack.packb(
                        version_props("blob:v2", valid_from=20, tx_from=200)
                    ),
                },
            )
        ],
    )

    results = clean_graph.changes.apply_batch([first, second])
    assert [result["result"] for result in results] == ["applied", "applied"], results

    # Both versions stay readable exactly as written: no window closed on v1
    # and neither names the other.
    props_first = clean_graph.nodes.properties(v1_id)
    props_second = clean_graph.nodes.properties(v2_id)
    assert props_first is not None and props_second is not None
    for props in (props_first, props_second):
        assert "valid_to" not in props, props
        assert "supersedes_id" not in props, props
    assert props_first["valid_from"] == 10 and props_second["valid_from"] == 20
    assert clean_graph.edges.properties(v2_id, v1_id) == []

    # A compare-and-set that would close v1's window refuses: a published
    # version never changes.
    close_window = _envelope(
        graph,
        v1_id,
        "h2-r801:v1-close",
        3,
        [
            op(
                0,
                "CompareAndSetNodeFields",
                {
                    "node_id": v1_id,
                    "conditions_msgpack": msgpack.packb({"confidence": 0.8}),
                    "updates_msgpack": msgpack.packb({"confidence": 0.1}),
                },
            )
        ],
    )
    try:
        clean_graph.changes.apply(close_window)
    except EngineResponseError as error:
        assert error.code == "HarnessVersionImmutable", error.code
    else:
        raise AssertionError("a compare-and-set on a published version must refuse")
    assert clean_graph.nodes.properties(v1_id)["confidence"] == 0.8

    # A write that would mutate the published v1 refuses with the
    # vocabulary's immutability code, and nothing is written.
    mutated = version_props("blob:v1", valid_from=10, tx_from=300, confidence=0.9)
    attempt = _envelope(
        graph,
        v1_id,
        "h2-r801:v1-mutated",
        3,
        [
            op(
                0,
                "AddNode",
                {
                    "node_id": v1_id,
                    "properties_msgpack": msgpack.packb(mutated),
                },
            )
        ],
    )
    try:
        clean_graph.changes.apply(attempt)
    except EngineResponseError as error:
        assert error.code == "HarnessVersionImmutable", error.code
    else:
        raise AssertionError("a mutated published version must refuse")
    assert clean_graph.nodes.properties(v1_id)["confidence"] == 0.8
