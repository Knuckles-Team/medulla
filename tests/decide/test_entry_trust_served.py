"""MEDULLA-DECISION-ENGINE-R132.6 served proof of entry trust.

The entry point is the wire client. A pinned policy knows one source digest. A
``Decide`` carrying a verified fact and an unverified entry that contradicts it
through the ``retrieved_entries`` param records the verified fact as a premise
and the unverified entry in ``refused_entries``; the unverified value never
becomes a premise.
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

SOURCE = "sha256:known-source"
SUBJECT = "urn:agent:planner"


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
        "entry_trust": {"known_source_digests": [SOURCE]},
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


def _entry(entry_id: str, tier: str, value: str) -> dict[str, Any]:
    return {
        "entry_id": entry_id,
        "subject": SUBJECT,
        "fact_key": "max_tools",
        "value": value,
        "trust": {"tier": tier, "source_digest": SOURCE, "observed_at": 10},
    }


async def _decide(
    schema: dict[str, Any], policy: dict[str, Any], entries: list[dict[str, Any]]
) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        request = dc.decide_request(
            TEST_TENANT,
            ("entry.trust", "path_preference", "ordinary"),
            dc.declared([dc.DeclaredOption("option-a", numbers={"score": 0})]),
            schema,
            policy=policy,
            params=[
                {
                    "name": "retrieved_entries",
                    "value": {"type": "retrieved_entries", "value": entries},
                }
            ],
        )
        payload = await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()
    return payload["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R132.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R132.6")
def test_unverified_entry_cannot_override_verified(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    schema = publish(
        clean_graph,
        f"r132-6-schema-{suffix}",
        "feature_schema",
        ds.feature_schema_body([ds.feature("score", ds.number("score"))]),
    )
    policy = {
        "policy": "pinned",
        "component": _publish_policy(clean_graph, f"r132-6-policy-{suffix}"),
    }

    record = asyncio.run(
        _decide(
            schema,
            policy,
            [
                _entry("e-verified", "verified", "8"),
                _entry("e-unverified", "unverified", "99"),
            ],
        )
    )
    facts = [premise["fact"] for premise in record["premises"]]
    assert "max_tools=8" in facts
    assert "max_tools=99" not in facts
    refused = record["refused_entries"]
    assert [(r["entry_id"], r["tier"], r["check"]) for r in refused] == [
        ("e-unverified", "unverified", "poison_check")
    ]
