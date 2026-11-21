"""MEDULLA-DECISION-ENGINE-R131.3 served proof of the limit stage.

The entry point is the wire client. A pinned policy carries one limit; a
``Decide`` whose ``guarded_action`` param stays inside it answers advisory, and
one that breaches it, or whose request bound tightens it, is withheld with the
``LIMIT_BREACH`` code and a ``limit:<rule>`` premise. A question without the
param passes unchanged.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publication, publish
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient
from medulla.decision import policy_digest

RULE = "max-position"
SCOPE = "account:a"


def _policy_body() -> dict[str, Any]:
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
        "limits": [
            {"id": RULE, "metric": "position_size", "bound": 10000, "scope": SCOPE}
        ],
    }


def _publish_policy(client: Any, component_id: str) -> dict[str, Any]:
    body = _policy_body()
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


def _action(observed: int, requested: int | None = None) -> dict[str, Any]:
    action: dict[str, Any] = {
        "signal_iri": "urn:signal:buy",
        "observations": [
            {"metric": "position_size", "scope": SCOPE, "value": observed}
        ],
    }
    if requested is not None:
        action["requested_bounds"] = [{"rule_id": RULE, "bound": requested}]
    return action


async def _decide(
    schema: dict[str, Any], policy: dict[str, Any], action: dict[str, Any] | None
) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        params = (
            []
            if action is None
            else [
                {
                    "name": "guarded_action",
                    "value": {"type": "guarded_action", "value": action},
                }
            ]
        )
        request = dc.decide_request(
            TEST_TENANT,
            ("limit.guard", "path_preference", "ordinary"),
            dc.declared([dc.DeclaredOption("option-a", numbers={"score": 0})]),
            schema,
            policy=policy,
            params=params,
        )
        payload = await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()
    return payload["records"][0]


def _code(record: dict[str, Any]) -> str:
    outcome = record["outcome"]
    assert outcome["outcome"] == "abstained", outcome
    return outcome["reasons"][0]["code"]


# spec: MEDULLA-DECISION-ENGINE-R131.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R131.3")
def test_breach_refuses(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    schema = publish(
        clean_graph,
        f"r131-3-schema-{suffix}",
        "feature_schema",
        ds.feature_schema_body([ds.feature("score", ds.number("score"))]),
    )
    policy = {
        "policy": "pinned",
        "component": _publish_policy(clean_graph, f"r131-3-policy-{suffix}"),
    }

    inside = asyncio.run(_decide(schema, policy, _action(10_000)))
    assert inside["outcome"]["outcome"] == "advisory", inside["outcome"]

    breach = asyncio.run(_decide(schema, policy, _action(10_001)))
    assert _code(breach) == "LIMIT_BREACH"
    assert any(p["subject"] == f"limit:{RULE}" for p in breach["premises"])

    # A request bound tightens the policy bound; it cannot loosen it.
    tight = asyncio.run(_decide(schema, policy, _action(6_000, requested=5_000)))
    assert _code(tight) == "LIMIT_BREACH"
    loose = asyncio.run(_decide(schema, policy, _action(12_000, requested=50_000)))
    assert _code(loose) == "LIMIT_BREACH"

    # No action param: the policy's limits check nothing.
    none = asyncio.run(_decide(schema, policy, None))
    assert none["outcome"]["outcome"] == "advisory", none["outcome"]
