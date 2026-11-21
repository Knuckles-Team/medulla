"""MEDULLA-DECISION-ENGINE-R173.6 served receipt: the edit concentration cap
over the wire.

The entry point is the served ``Decide`` method with the
``edit_concentration`` question kind. The caller declares the two action
options and names the graph that holds the edits, the candidate edit and the
policy node as typed request params. The engine reads the caller's filtered
snapshot of that graph, counts the shipped edits on the candidate's dimension
inside the policy window and answers ``action:revert`` when the count with the
candidate reaches the policy's ``minShipped``, otherwise ``action:keep``.

The edits, the dimension and the policy are written through the generated
client write (``ApplyChangeEnvelopes``; ``changes.apply_batch`` compiles to
it), the same entry point as the contrast tests.
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import publish
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient

DIMENSION = "dimension:tool-selection"
POLICY = "policy:concentration"
#: Shipped edits in rounds 2 to 6 on the one dimension.
SHIPPED_ROUNDS = [2, 3, 4, 5, 6]
MIN_SHIPPED = 3
WINDOW_ROUNDS = 5


def edit_id(round_number: int) -> str:
    return f"edit:round-{round_number}"


def _apply(clean_graph: Any, key: str, ops: list[dict[str, Any]]) -> None:
    envelope = _envelope(_graph_name(clean_graph), key, key, 1, ops)
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["result"] for result in results] == ["applied"], results


async def _create_edits(graph: str) -> None:
    """One shipped edit per round: created and shipped through ``HarnessEdit``, the only
    writer of ``editStatus`` (R173.8). Each edit points at the dimension with a
    ``targetsDimension`` edge, which ``Create`` writes."""
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client, graph=graph)
        for round_number in SHIPPED_ROUNDS:
            created = await decision.harness_edit(
                {
                    "op": "create",
                    "edit_id": edit_id(round_number),
                    "dimension_iri": DIMENSION,
                    "round": round_number,
                },
                idempotency_key=f"r173-6:create:{round_number}",
            )
            assert created["status"] == "proposed", created
            shipped = await decision.harness_edit(
                {
                    "op": "transition",
                    "edit_id": edit_id(round_number),
                    "event": "ship",
                },
                idempotency_key=f"r173-6:ship:{round_number}",
            )
            assert shipped["status"] == "shipped", shipped
    finally:
        await client.close()


def _write_edits(clean_graph: Any) -> None:
    """The dimension and the policy through the generated client write, then the shipped
    edits through ``HarnessEdit``."""
    nodes: list[tuple[str, dict[str, Any]]] = [
        (DIMENSION, {}),
        (
            POLICY,
            {
                "minShipped": MIN_SHIPPED,
                "windowRounds": WINDOW_ROUNDS,
                "revision": 1,
            },
        ),
    ]
    ops = [
        op(
            ordinal,
            "AddNode",
            {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        )
        for ordinal, (node_id, props) in enumerate(nodes)
    ]
    _apply(clean_graph, "r173-6:concentration:edits", ops)
    asyncio.run(_create_edits(_graph_name(clean_graph)))


def _options() -> list[dc.DeclaredOption]:
    return [dc.DeclaredOption("action:keep"), dc.DeclaredOption("action:revert")]


def _params(graph: str, candidate: str) -> list[dict[str, Any]]:
    return [
        dc.param("candidate_edit", "iri", candidate),
        dc.param("concentration_policy", "iri", POLICY),
        dc.param("graph", "iri", graph),
    ]


async def _decide(
    schema: dict[str, Any], question_id: str, graph: str, candidate: str
) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        request = dc.decide_request(
            TEST_TENANT,
            (question_id, "edit_concentration", "ordinary"),
            dc.declared(_options()),
            schema,
            params=_params(graph, candidate),
        )
        payload = await decision.decide(request)
        return payload["records"][0]
    finally:
        await client.close()


def _verdict(record: dict[str, Any]) -> tuple[str, str]:
    """The action the advisory chose (the option that scores one) and the
    ``count:<n>`` option it carries."""
    assert record["outcome"]["outcome"] == "advisory", record["outcome"]
    scores = record["outcome"]["scores"]
    chosen = [
        s["option_id"]
        for s in scores
        if s["option_id"].startswith("action:") and s["score"]["value"] != 0
    ]
    counts = [s["option_id"] for s in scores if s["option_id"].startswith("count:")]
    assert len(chosen) == 1 and len(counts) == 1, scores
    return chosen[0], counts[0]


# spec: MEDULLA-DECISION-ENGINE-R173.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R173.6")
def test_third_same_dimension_edit_reverts(clean_graph):
    _write_edits(clean_graph)
    schema = publish(
        clean_graph,
        "r173-6-schema",
        "feature_schema",
        ds.feature_schema_body(
            [ds.feature("action_value", ds.number("action_value"), impute=0)]
        ),
    )
    graph = _graph_name(clean_graph)

    # Round 4 is the third edit on the dimension inside the window (rounds 2
    # and 3 shipped before it): the count reaches minShipped, so it reverts.
    third = asyncio.run(_decide(schema, "q:r173-6:third", graph, edit_id(4)))
    assert third["question"]["kind"] == "edit_concentration"
    assert _verdict(third) == ("action:revert", "count:3")

    # Round 3 is the second edit: one shipped edit before it, count two.
    second = asyncio.run(_decide(schema, "q:r173-6:second", graph, edit_id(3)))
    assert _verdict(second) == ("action:keep", "count:2")

    # The window is five rounds wide: round 6's window starts at round 2, so
    # all four earlier shipped edits count with it.
    last = asyncio.run(_decide(schema, "q:r173-6:last", graph, edit_id(6)))
    assert _verdict(last) == ("action:revert", "count:5")

    # Decide is evaluate-only: the same question answers the same again.
    again = asyncio.run(_decide(schema, "q:r173-6:third", graph, edit_id(4)))
    assert _verdict(again) == ("action:revert", "count:3")
