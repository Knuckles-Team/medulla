"""MEDULLA-DECISION-ENGINE-R158.6 served proof of run adjustment.

The entry point is the wire client: the run loop writes one
``RunObservationWindow`` node per window through ``add_node``, then the
tenant's caller sends ``Decide`` with the ``run_adjustment`` question kind.
The engine reads the run's windows from the graph, counts each as one run and
answers the model tier, width and token budget of the next runs: a high error
rate moves the model up a tier and narrows the width; a low one widens the
width up to the host ceiling and keeps the model.
"""

from __future__ import annotations

import asyncio
import json
import os
import time
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publication, publish, schema_body
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient
from medulla.decision import policy_digest

Q32 = 1 << 32
RUNS = 20
CEILING = 8
BUDGET = 1_000


def _policy_body(error_hi_micros: int, error_lo_micros: int) -> dict[str, Any]:
    """The engine default policy with the two error thresholds pinned."""
    return {
        "schema_version": 1,
        "objective": {
            "order": "lexicographic",
            "levels": [
                "uncovered",
                "components",
                "declared_cost",
                "declared_p95_latency",
            ],
        },
        "unknown_cost": "exclude_when_strict",
        "accepted_gap": "0",
        "node_budget": 100000,
        "max_templates": 8,
        "max_slots": 6,
        "max_nogood_rounds": 8,
        "max_why_not_per_slot": 3,
        "a2a_requires_observation": True,
        "cold_start": {"mode": "advisory_uncalibrated"},
        "statistical": None,
        "knobs": {
            "width.error_hi": {"type": "decimal_micros", "value": error_hi_micros},
            "width.error_lo": {"type": "decimal_micros", "value": error_lo_micros},
        },
    }


def _publish_policy(client: Any, component_id: str, body: dict[str, Any]) -> dict:
    request = publication(component_id, "decision_policy", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = policy_digest(body)
    component["attributes"] = {"decision.policy": json.dumps(body)}
    entry = client._send("AgentComponent", request)["component"]
    return {
        "component_id": component_id,
        "kind": "decision_policy",
        "definition_digest": entry["definition_digest"],
    }


async def _connect(scopes: list[str] | None = None) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(scopes=scopes),
    )


UNKNOWN = {"state": "unknown", "value": "not reported"}


def _measured(value: Any) -> dict[str, Any]:
    return {"state": "measured", "value": value}


def _append_runs(client: Any, run: str, runs: int, errored: int) -> None:
    """``runs`` windows of ``run``, the first ``errored`` of them with an error,
    all inside the observation window the engine reads."""
    now = int(time.time() * 1000)
    for i in range(runs):
        end_ms = now - 10_000 - i
        client.nodes.add(
            f"{run}-w{i}",
            {
                "node_type": "RunObservationWindow",
                "run_id": run,
                "run_revision": 1,
                "window_id": f"{run}-w{i}",
                "window_revision": 1,
                "start_ms": end_ms - 100,
                "end_ms": end_ms,
                "latency_samples_ms": _measured([20] * 5),
                "error_count": _measured(1 if i < errored else 0),
                "eligible_attempts": _measured(4),
                "usage": {
                    "input_tokens": UNKNOWN,
                    "output_tokens": UNKNOWN,
                    "cache_read_tokens": UNKNOWN,
                    "cache_write_tokens": UNKNOWN,
                },
                "cost": UNKNOWN,
                "model_ref": UNKNOWN,
                "profile_revision": UNKNOWN,
                "width": UNKNOWN,
                "budget_consumed": UNKNOWN,
                "budget_remaining": UNKNOWN,
                "capacity_receipt_ref": UNKNOWN,
            },
        )


def _adjustment_request(
    schema: dict[str, Any],
    graph: str,
    run: str,
    current_model: str,
    current_width: int,
    policy: dict[str, Any] | None = None,
) -> dict[str, Any]:
    models = [
        dc.DeclaredOption(model, numbers={"tier": tier * Q32})
        for tier, model in enumerate(["m1", "m2", "m3"], start=1)
    ]
    return dc.decide_request(
        TEST_TENANT,
        (f"q:r158-6:{run}", "run_adjustment", "ordinary"),
        dc.declared(models),
        schema,
        policy=policy,
        params=[
            dc.param("run_id", "text", run),
            dc.param("graph", "text", graph),
            dc.param("current_model", "iri", current_model),
            dc.param("current_width", "int", current_width),
            dc.param("host_width_ceiling", "int", CEILING),
            dc.param("budget_tokens", "int", BUDGET),
            dc.param("mid_run", "bool", False),
        ],
    )


async def _adjust(request: dict[str, Any]) -> list[str]:
    client = await _connect()
    try:
        payload = await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()
    outcome = payload["records"][0]["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return sorted(score["option_id"] for score in outcome["scores"])


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R158.6")
def test_per_run_adjustment(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R158.6
    suffix = uuid.uuid4().hex[:8]
    noisy, quiet = f"noisy-{suffix}", f"quiet-{suffix}"
    schema = publish(
        clean_graph, f"r158-6-schema-{suffix}", "feature_schema", schema_body()
    )

    # 5 of 20 runs errored: the next tier up and one less width.
    graph = _graph_name(clean_graph)
    _append_runs(clean_graph, noisy, RUNS, 5)
    high = asyncio.run(_adjust(_adjustment_request(schema, graph, noisy, "m1", 4)))
    assert high == [f"budget_tokens:{BUDGET}", "model:m2", "width:3"]

    # One errored run of 20 is 5%: a policy that tolerates up to 6% widens the
    # width one step and keeps the model, and stops at the host ceiling.
    _append_runs(clean_graph, quiet, RUNS, 1)
    tolerant = {
        "policy": "pinned",
        "component": _publish_policy(
            clean_graph,
            f"r158-6-policy-{suffix}",
            _policy_body(error_hi_micros=200_000, error_lo_micros=60_000),
        ),
    }
    wider = asyncio.run(
        _adjust(_adjustment_request(schema, graph, quiet, "m2", 4, policy=tolerant))
    )
    assert wider == [f"budget_tokens:{BUDGET}", "model:m2", "width:5"]
    capped = asyncio.run(
        _adjust(
            _adjustment_request(schema, graph, quiet, "m2", CEILING, policy=tolerant)
        )
    )
    assert capped == [f"budget_tokens:{BUDGET}", "model:m2", f"width:{CEILING}"]
