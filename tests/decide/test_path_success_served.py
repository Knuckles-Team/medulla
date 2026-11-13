"""MEDULLA-DECISION-ENGINE-R159.6 served proof of path preference.

The entry point is the wire client. A decider routes one decision per path
under a policy that permits exploration, commits the record naming an
independent evaluator, and that evaluator (a second caller) records one
outcome per attempt through ``DecisionLog`` ``Evaluate``. Then ``Decide`` with
the ``path_preference`` question kind ranks the declared paths by their recent
success rate and skips a path whose latest three outcomes all failed. ``Decide``
writes nothing and no new method is involved.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import time
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publication, publish
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient
from medulla.decision import policy_digest

Q32 = 1 << 32
QUESTION = "route.paths"
EVALUATOR = "path-evaluator"
HOUR_MS = 3_600_000


def _path_id(connector: str, method: str) -> str:
    """``sha256(connector 0x00 method 0x00 parameter_digest_hex)``, the
    engine's one routing-path identity."""
    parameter_digest = hashlib.sha256(b"params").hexdigest()
    return hashlib.sha256(
        connector.encode()
        + b"\x00"
        + method.encode()
        + b"\x00"
        + parameter_digest.encode()
    ).hexdigest()


def _exploring_policy_body() -> dict[str, Any]:
    """The engine default policy, except that exploration is permitted for the
    routing question, so each seeded decision is a logged exploration."""
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
        "cold_start": {
            "mode": "explore",
            "budget": {
                "fraction": {"numerator": 1, "denominator": 1},
                "spend_at_risk_micros": 0,
                "questions": [QUESTION],
            },
        },
        "statistical": None,
    }


def _publish_policy(client: Any, component_id: str) -> dict[str, Any]:
    body = _exploring_policy_body()
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


def _schema_body() -> dict[str, Any]:
    return ds.feature_schema_body([ds.feature("score", ds.number("score"))])


def _path_option(path: str) -> dc.DeclaredOption:
    return dc.DeclaredOption(path, numbers={"score": 0})


async def _connect(agent_id: str | None = None) -> MedullaClient:
    context = (
        request_context() if agent_id is None else request_context(agent_id=agent_id)
    )
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"], verified_context=context
    )


def _evaluation(record_id: str, index: int, success: bool) -> dict[str, Any]:
    return {
        "record_id": record_id,
        "evaluation_id": f"e-{index:03}",
        "class": "observation",
        "selected_agent": "selected-agent",
        "lease_holder": "lease-holder",
        "fidelity": "tool_calls",
        "success": success,
        "backbone": None,
    }


async def _decide_and_commit(
    schema: dict[str, Any], policy: dict[str, Any], path: str
) -> str:
    """The decider's logged decision on ``path``, naming the evaluator."""
    evaluator = "principal:sha256:" + hashlib.sha256(EVALUATOR.encode()).hexdigest()
    client = await _connect()
    try:
        decision = dc.DecisionClient(client=client)
        request = dc.decide_request(
            TEST_TENANT,
            (QUESTION, "route", "ordinary"),
            dc.declared([_path_option(path)]),
            schema,
            policy=policy,
        )
        record = (await decision.decide(request))["records"][0]
        assert record["outcome"]["outcome"] == "explored", record["outcome"]
        named = dc.named_evaluator(
            int(time.time() * 1000) + HOUR_MS, principal=evaluator
        )
        await decision.log(dc.commit_op(record, named))
        return record["record_id"]
    finally:
        await client.close()


async def _evaluate(record_id: str, first_index: int, outcomes: list[bool]) -> None:
    """One independent evaluation per outcome, oldest first, by the evaluator."""
    client = await _connect(EVALUATOR)
    try:
        decision = dc.DecisionClient(client=client)
        for offset, success in enumerate(outcomes):
            await decision.log(
                dc.evaluate_op(
                    TEST_TENANT, _evaluation(record_id, first_index + offset, success)
                )
            )
    finally:
        await client.close()


async def _seed(
    schema: dict[str, Any], policy: dict[str, Any], path: str, outcomes: list[bool]
) -> str:
    record_id = await _decide_and_commit(schema, policy, path)
    await _evaluate(record_id, 0, outcomes)
    return record_id


async def _prefer(schema: dict[str, Any], paths: list[str]) -> list[str]:
    client = await _connect()
    try:
        request = dc.decide_request(
            TEST_TENANT,
            ("path.preference", "path_preference", "ordinary"),
            dc.declared([_path_option(path) for path in paths]),
            schema,
        )
        payload = await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()
    outcome = payload["records"][0]["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return [score["option_id"] for score in outcome["scores"]]


# spec: MEDULLA-DECISION-ENGINE-R159.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R159.6")
def test_success_history_drives_preference(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    connector = f"conn:{suffix}"
    path_a, path_b = _path_id(connector, "a"), _path_id(connector, "b")
    schema = publish(
        clean_graph, f"r159-6-schema-{suffix}", "feature_schema", _schema_body()
    )
    policy = {
        "policy": "pinned",
        "component": _publish_policy(clean_graph, f"r159-6-policy-{suffix}"),
    }

    # A: 9 successes in 10 attempts, its one failure in the middle.
    a_outcomes = [True] * 4 + [False] + [True] * 5
    # B: 3 successes in 10 attempts, the latest of them a failure but not
    # three in a row.
    b_outcomes = [False] * 6 + [True] * 3 + [False]
    asyncio.run(_seed(schema, policy, path_a, a_outcomes))
    b_record = asyncio.run(_seed(schema, policy, path_b, b_outcomes))

    # A is preferred: its 90% beats B's 30%, and B is still ranked.
    assert asyncio.run(_prefer(schema, [path_a, path_b])) == [path_a, path_b]

    # Two more failures: B's latest three outcomes fail, so B is skipped.
    asyncio.run(_evaluate(b_record, len(b_outcomes), [False, False]))
    assert asyncio.run(_prefer(schema, [path_a, path_b])) == [path_a]
