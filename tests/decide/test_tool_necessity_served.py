"""MEDULLA-DECISION-ENGINE-R165.6 served proof of tool necessity and mismatch.

The entry point is the wire client. The evidence is 30 run traces that used the
tool (all succeeded) and 30 that did not (6 succeeded), as ``RunTrace`` nodes
of the evidence graph written through the generated client write. The tool and
the model profile are published components. ``Decide`` with the
``tool_necessity`` question kind then measures the tool's necessity (the
success rate with the tool minus the success rate without it) and compares it
with what the model did: once with ``model_called`` true (agreement, no
mismatch) and once false (a ``missed_tool`` mismatch linked to the recorded
``model_call_id``). ``Decide`` writes nothing.
"""

from __future__ import annotations

import asyncio
import os
import time
import uuid
from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import publication, publish
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient

Q32 = 1 << 32
WITH_TOOL, WITHOUT_TOOL = 30, 30
WITHOUT_TOOL_SUCCEEDED = 6


def _schema_body() -> dict[str, Any]:
    return ds.feature_schema_body([ds.feature("score", ds.number("score"))])


def _publish_known(client: Any, tool_id: str, model_id: str) -> None:
    """The published Tool and ModelProfile components the question names."""
    tool = publication(tool_id, "tool", {})
    tool["op"]["request"]["component"].update(attributes={}, summary="search")
    client._send("AgentComponent", tool)
    model = publication(model_id, "model_profile", {})
    model["op"]["request"]["component"].update(
        attributes={},
        facts={
            "facts": "model_profile",
            "provider": "fixture",
            "model_identity": model_id,
            "context_window_tokens": 8192,
            "max_output_tokens": 1024,
            "supports_tools": True,
            "supports_structured_output": True,
            "supports_vision": False,
        },
    )
    client._send("AgentComponent", model)


def _trace_nodes(
    tag: str, tool_id: str, model_id: str, now_ms: int
) -> list[tuple[str, dict[str, Any]]]:
    """60 run-trace nodes: 30 used the tool and all succeeded; 30 did not and
    6 of them succeeded."""
    arms = [(True, index, True) for index in range(WITH_TOOL)] + [
        (False, index, index < WITHOUT_TOOL_SUCCEEDED) for index in range(WITHOUT_TOOL)
    ]
    nodes = []
    for position, (used, index, succeeded) in enumerate(arms):
        run_id = f"run:{tag}:{'with' if used else 'without'}:{index}"
        steps = (
            [
                {
                    "index": 0,
                    "kind": "tool_call",
                    "action_ref": tool_id,
                    "outcome": "ok",
                }
            ]
            if used
            else []
        )
        nodes.append(
            (
                f"trace:{tag}:{position}",
                {
                    "kind": "run_trace",
                    "run_trace": {
                        "schema_version": 1,
                        "run_id": run_id,
                        "task_signature": "task:necessity",
                        "model_id": model_id,
                        "steps": steps,
                        "outcome": "succeeded" if succeeded else "failed",
                        "started_at_ms": now_ms,
                    },
                },
            )
        )
    return nodes


def _seed(clean_graph: Any, tag: str, nodes: list[tuple[str, dict[str, Any]]]) -> None:
    envelope = _envelope(
        _graph_name(clean_graph),
        tag,
        tag,
        1,
        [
            op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ],
    )
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["result"] for result in results] == ["applied"], results


async def _decide(
    schema: dict[str, Any],
    graph: str,
    tool_id: str,
    model_id: str,
    called: bool,
) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        request = dc.decide_request(
            TEST_TENANT,
            ("tool.necessity", "tool_necessity", "ordinary"),
            dc.declared([dc.DeclaredOption("tool", numbers={"score": 0})]),
            schema,
            params=[
                dc.param("graph", "text", graph),
                dc.param("model_call_id", "text", "call:1"),
                dc.param("model_called", "bool", called),
                dc.param("model_id", "iri", model_id),
                dc.param("tool_id", "iri", tool_id),
            ],
        )
        payload = await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()
    return payload["records"][0]


def _options(record: dict[str, Any]) -> list[str]:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return [score["option_id"] for score in outcome["scores"]]


# spec: MEDULLA-DECISION-ENGINE-R165.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R165.6")
def test_necessity_mismatch_trace(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    tool_id, model_id = f"tool:search-{suffix}", f"model:necessity-{suffix}"
    schema = publish(
        clean_graph, f"r165-6-schema-{suffix}", "feature_schema", _schema_body()
    )
    _publish_known(clean_graph, tool_id, model_id)
    _seed(
        clean_graph,
        f"r165-6:{suffix}",
        _trace_nodes(suffix, tool_id, model_id, int(time.time() * 1000)),
    )
    graph = _graph_name(clean_graph)

    # With the tool every run succeeded (30/30); without it 6/30: 1 - 0.2.
    expected = Q32 - (Q32 * WITHOUT_TOOL_SUCCEEDED) // WITHOUT_TOOL

    skipped = asyncio.run(_decide(schema, graph, tool_id, model_id, False))
    options = _options(skipped)
    necessity = int(options[0].removeprefix("necessity:"))
    assert abs(necessity - expected) <= 1, options
    assert options[1:] == ["agreement:false", "mismatch:missed_tool"]
    premises = {premise["subject"]: premise for premise in skipped["premises"]}
    assert premises["tool_necessity.samples_with"]["fact"] == str(WITH_TOOL)
    assert premises["tool_necessity.samples_without"]["fact"] == str(WITHOUT_TOOL)
    assert premises["tool_necessity.comparison"]["fact"] == "false"
    mismatch = premises["tool_necessity.mismatch:missed_tool"]
    assert mismatch["fact"] == "call:1"

    called = asyncio.run(_decide(schema, graph, tool_id, model_id, True))
    options = _options(called)
    assert options[1:] == ["agreement:true"]
    assert not any(
        subject.startswith("tool_necessity.mismatch")
        for subject in (premise["subject"] for premise in called["premises"])
    )
