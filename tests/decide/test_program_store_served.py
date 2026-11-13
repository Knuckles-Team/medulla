"""MEDULLA-DECISION-ENGINE-R135.6 served compiled programs: a successful run is
committed through ``DecisionLog`` (``run_record``) and its program is looked up
through ``Decide`` (``program_lookup``) over the wire.

The source ``RunTrace`` receipt node is written through the generated client
write (``ApplyChangeEnvelopes``), as the terminal commit leaves it. The commit
compiles the run into a program keyed by task signature and links it to the
trace node; the lookup returns the stored program, equal to the committed one.
A failed run commits nothing.
"""

from __future__ import annotations

import asyncio
import json
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

SIGNATURE = "summarize-report"


def _trace(run_id: str, outcome: str, steps: int) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "run_id": run_id,
        "task_signature": SIGNATURE,
        "model_id": "model-1",
        "steps": [
            {
                "index": index,
                "kind": "tool_call",
                "action_ref": f"tool:t{index}",
                "outcome": "ok",
                "arguments": [
                    {"name": "title", "value": "weekly"},
                    {"name": "limit", "value": 10},
                ],
            }
            for index in range(steps)
        ],
        "outcome": outcome,
        "started_at_ms": 1,
    }


def _write_trace_node(clean_graph: Any, key: str, node_id: str, trace: dict) -> None:
    props = {"type": "RunTrace", "kind": "run_trace", "run_trace": trace}
    envelope = _envelope(
        _graph_name(clean_graph),
        key,
        key,
        1,
        [
            op(
                0,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
        ],
    )
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results


def _run_record_op(trace: dict[str, Any], graph: str, node_id: str) -> dict[str, Any]:
    return {
        "op": "run_record",
        "tenant_id": TEST_TENANT,
        "record": {
            "kind": "compiled_program",
            "trace": trace,
            "policy": {"policy": "default"},
            "source": {"graph": graph, "node_id": node_id},
        },
    }


def _program_from(payload: dict[str, Any]) -> dict[str, Any]:
    record = payload["records"][0]
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    option_id = outcome["scores"][0]["option_id"]
    assert option_id.startswith("program:")
    return json.loads(option_id.removeprefix("program:"))


async def _journey(schema: dict[str, Any], graph: str, good: str, bad: str) -> dict:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)

        def lookup(signature: str) -> dict[str, Any]:
            return dc.decide_request(
                TEST_TENANT,
                (f"q:program:{signature}", "program_lookup", "ordinary"),
                dc.declared([dc.DeclaredOption("program")]),
                schema,
                params=[dc.param("task_signature", "text", signature)],
            )

        before = await decision.decide(lookup(SIGNATURE))
        committed = await decision.log(
            _run_record_op(_trace("run-ok", "succeeded", 3), graph, good)
        )
        found = await decision.decide(lookup(SIGNATURE))
        try:
            await decision.log(
                _run_record_op(_trace("run-bad", "failed", 2), graph, bad)
            )
            refused = None
        except Exception as error:  # the typed refusal carries the code
            refused = str(error)
        after = await decision.decide(lookup(SIGNATURE))
        return {
            "before": before,
            "committed": committed,
            "found": found,
            "refused": refused,
            "after": after,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R135.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R135.6")
def test_compile_then_lookup(clean_graph):
    good = _trace("run-ok", "succeeded", 3)
    bad = _trace("run-bad", "failed", 2)
    _write_trace_node(clean_graph, "r135-6:trace:ok", "trace:run-ok", good)
    _write_trace_node(clean_graph, "r135-6:trace:bad", "trace:run-bad", bad)
    schema = publish(
        clean_graph,
        "r135-6-schema",
        "feature_schema",
        ds.feature_schema_body([ds.feature("n", ds.number("n"))]),
    )
    result = asyncio.run(
        _journey(schema, _graph_name(clean_graph), "trace:run-ok", "trace:run-bad")
    )

    # Nothing is committed yet: the lookup abstains.
    assert result["before"]["records"][0]["outcome"]["outcome"] == "abstained"

    # The commit stored the program and linked it to its source trace node.
    committed = result["committed"]
    assert committed["replayed"] is False
    assert committed["key"] == f"run_record/compiled_program/{SIGNATURE}"
    stored = committed["stored"]
    assert stored["kind"] == "compiled_program"
    assert stored["program_node"] == f"program:{SIGNATURE}"

    # The second call returns the program the first committed, parameters and all.
    program = _program_from(result["found"])
    assert program == stored["program"]
    assert program["task_signature"] == SIGNATURE
    assert program["source_run_id"] == "run-ok"
    assert [parameter["name"] for parameter in program["parameters"]] == ["p0", "p1"]
    assert len(program["steps"]) == 3

    # A failed trace commits nothing: the stored program is still the first.
    assert result["refused"] is not None
    assert "RUN_NOT_SUCCESSFUL" in result["refused"]
    assert _program_from(result["after"]) == program
