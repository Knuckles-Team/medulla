"""MEDULLA-DECISION-ENGINE-R130.6 served calibrated ranking: a listwise head is
fitted, evaluated and published over the wire, then `Decide` ranks tied options
by the caller's item score and seals the tie-break premises."""

from __future__ import annotations

import json
from typing import Any

import pytest
from _decision_resident_fixture import job_requests, publication, publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.decision import policy_digest

Q32 = 1 << 32
IDS = ["opt-a", "opt-b", "opt-c"]


def _advisory_policy_body() -> dict[str, Any]:
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
    }


def _publish_policy(client: Any) -> dict[str, Any]:
    body = _advisory_policy_body()
    request = publication("rank-policy", "decision_policy", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = policy_digest(body)
    component["attributes"] = {"decision.policy": json.dumps(body)}
    entry = client._send("AgentComponent", request)["component"]
    return {
        "component_id": "rank-policy",
        "kind": "decision_policy",
        "definition_digest": entry["definition_digest"],
    }


def _dataset(schema_body: dict[str, Any]) -> dict[str, Any]:
    return dict(
        schema_version=1,
        feature_schema_digest=ds.encode_body(schema_body)[0],
        feature_names=["signal"],
        scale="q32",
        items=[
            dict(
                item_id=f"gold-{i:04}",
                recorded_at_ms=1000,
                class_key="rank",
                candidate_ids=IDS,
                features=[Q32 + (((i % 7) - 3) << 24), 0, 0],
                label={
                    "label": "gold",
                    "acceptable": [IDS[0]],
                    "source": "synthetic_construction",
                },
                audit_inclusion=None,
            )
            for i in range(400)
        ],
        synthetic=True,
    )


def _publish_head(client: Any) -> tuple[dict[str, Any], dict[str, Any]]:
    body = ds.feature_schema_body([ds.feature("signal", ds.number("signal"))])
    schema = publish(client, "rank-schema", "feature_schema", body)
    data = _dataset(body)
    fit = job_requests(schema, data)
    fit["op"]["request"]["head_kind"] = "listwise_logistic"
    fitted = client._send("DecisionFit", fit)
    assert fitted["state"]["state"] == "succeeded", fitted
    draft = fitted["state"]["output"]
    assert draft["draft"]["kind"] == "listwise_logistic"
    evaluated = client._send("DecisionEval", job_requests(schema, data, draft))
    assert evaluated["state"]["state"] == "succeeded", evaluated
    receipt = evaluated["state"]["output"]["receipt"]
    assert receipt["passed"], receipt["failed_gates"]
    head = publish(
        client, "rank-head", "decision_head", draft["draft"], receipt["receipt_digest"]
    )
    return schema, head


def _options(item_scores: dict[str, int]) -> Any:
    signals = {"opt-a": Q32, "opt-b": 0, "opt-c": 0}
    codes = {"opt-a": "code-a", "opt-b": "code-tied", "opt-c": "code-tied"}
    return dc.declared(
        [
            dc.DeclaredOption(
                name,
                numbers={"signal": signals[name], "rank.item_score": item_scores[name]},
                texts={"rank.code": codes[name]},
            )
            for name in IDS
        ]
    )


def _decide(client, schema, head, policy, item_scores):
    request = dc.decide_request(
        TEST_TENANT,
        ("rank.options", "rank", "ordinary"),
        _options(item_scores),
        schema,
        head=head,
        policy={"policy": "pinned", "component": policy},
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


def _order(record: dict[str, Any]) -> list[str]:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    assert outcome["calibrated"] is True
    return [score["option_id"] for score in outcome["scores"]]


# spec: MEDULLA-DECISION-ENGINE-R130.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R130.6")
def test_tied_codes_resolve_by_item_score(clean_graph):
    schema, head = _publish_head(clean_graph)
    policy = _publish_policy(clean_graph)
    record = _decide(
        clean_graph,
        schema,
        head,
        policy,
        {"opt-a": 0, "opt-b": Q32 // 10, "opt-c": Q32 * 8 // 10},
    )
    assert _order(record) == ["opt-a", "opt-c", "opt-b"]
    facts = {(p["subject"], p["fact"]) for p in record["premises"]}
    assert ("opt-a", "tie_break_applied=false") in facts
    assert ("opt-b", "tie_break_applied=true") in facts
    assert ("opt-c", "tie_break_applied=true") in facts
    assert any(fact == "tie_break_policy=item_score" for _, fact in facts)

    swapped = _decide(
        clean_graph,
        schema,
        head,
        policy,
        {"opt-a": 0, "opt-b": Q32 * 8 // 10, "opt-c": Q32 // 10},
    )
    assert _order(swapped) == ["opt-a", "opt-b", "opt-c"]
