"""MEDULLA-DECISION-ENGINE-R179.6 served knob journey.

One authenticated engine runs the whole path over the wire: a policy component
is published at two revisions (the second carries a changed knob), a ``Route``
decision under the policy's champion reads the engine default, the second
revision is promoted by the champion pointer (held-out policy evaluation, a
``Promotion`` decision, a committed record, then ``move_pointer``), the same
decision now reads the changed knob, and one rollback returns it.
"""

from __future__ import annotations

import json
from typing import Any

import pytest
from _decision_resident_fixture import publication, publish, schema_body
from conftest import TEST_TENANT

from decide.test_ranked_decide_served import _advisory_policy_body
from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)
from medulla import decision_client as dc
from medulla.decision import policy_digest

POLICY_ID = "knob-evolution-policy"
KNOB = "drift.shift_threshold"
CHANGED = {"type": "decimal_micros", "value": 400_000}
SUBJECT = {"kind": "knob_set", "policy_id": POLICY_ID}


def _publish_revision(
    client: Any, body: dict[str, Any], expected_revision: int
) -> dict[str, Any]:
    request = publication(POLICY_ID, "decision_policy", {})
    inner = request["op"]["request"]
    inner["context"]["expected_revision"] = expected_revision
    inner["context"]["idempotency_key"] = f"{POLICY_ID}:{expected_revision}"
    inner["component"]["content_digest"] = policy_digest(body)
    inner["component"]["attributes"] = {"decision.policy": json.dumps(body)}
    entry = client._send("AgentComponent", request)["component"]
    return {"content_digest": entry["content_digest"], "entry": entry}


def _route(client: Any, schema: dict[str, Any]) -> dict[str, Any]:
    asked = dc.decide_request(
        TEST_TENANT,
        ("route.knobs", "route", "ordinary"),
        dc.declared(
            [
                dc.DeclaredOption(
                    "tool-a-search",
                    texts={"summary": "web search engine for pages"},
                    classification=("eg:capability/retrieval/web-search",),
                )
            ]
        ),
        schema,
        policy={"policy": "champion", "policy_id": POLICY_ID},
        params=[
            dc.param("needs", "iri_list", ["eg:capability/retrieval"]),
            dc.param("query", "text", "web search engine"),
        ],
    )
    return client._send("Decide", {"request": asked})["records"][0]


def _knob_fact(record: dict[str, Any]) -> str:
    facts = [p["fact"] for p in record["premises"]]
    return next(f for f in facts if f.startswith(f"knob:{KNOB}="))


def _promote(client: Any, schema: dict[str, Any], target: str) -> str:
    """Promote ``target`` as the champion of the knob set; the record id."""
    evaluation: dict[str, Any] = {
        "subject": SUBJECT,
        "revision_ref": target,
        "champion_revision_ref": None,
        "evaluation_set_digest": "sha256:" + "10" * 32,
        "evaluator_digest": "sha256:" + "11" * 32,
        "evaluator_kind": "annotation",
        "held_out_trajectory_ids": [],
        "metrics": {
            "task_success_ppm": 900_000,
            "baseline_task_success_ppm": 850_000,
            "cost_micros": 12,
            "latency_p50_ms": 4,
            "latency_p95_ms": 9,
            "gpu_seconds": 2,
            "trace_completeness_ppm": 1_000_000,
            "unsupported_replay_mass_ppm": 0,
        },
        "safety": "passed",
        "verdict": "accepted",
        "report": None,
    }
    receipt = client._send(
        "PolicyEvolution",
        {"op": {"op": "commit_policy_evaluation", "request": evaluation}},
    )
    asked = dc.decide_request(
        TEST_TENANT,
        ("promote.knobs", "promotion", "ordinary"),
        dc.declared([dc.DeclaredOption(target)]),
        schema,
        params=[
            dc.param("mode", "text", "promote"),
            dc.param("receipt_digest", "text", receipt["record_id"]),
            dc.param("subject", "text", json.dumps(SUBJECT)),
        ],
    )
    record = client._send("Decide", {"request": asked})["records"][0]
    facts = [p["fact"] for p in record["premises"]]
    assert "promotion.verdict=promote" in facts, facts
    committed = client._send("DecisionLog", {"op": dc.commit_op(record)})
    return committed["record_id"]


def _move(client: Any, movement: dict[str, Any]) -> dict[str, Any]:
    return client._send(
        "DecisionLog",
        {
            "op": dc.learn_op(
                TEST_TENANT,
                "move_pointer",
                pointer={"pointer": "champion", "subject": SUBJECT},
                movement=movement,
            )
        },
    )


# spec: MEDULLA-DECISION-ENGINE-R179.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R179.6")
def test_promote_and_roll_back_a_knob(authenticated_plan_client):  # noqa: F811
    client = authenticated_plan_client
    try:
        schema = publish(
            client, "knob-evolution-schema", "feature_schema", schema_body()
        )
        base = _advisory_policy_body()
        first = _publish_revision(client, base, 0)
        changed = {**base, "knobs": {KNOB: CHANGED}}
        second = _publish_revision(client, changed, 1)

        # Nothing is promoted: the champion reference serves the engine default.
        default_fact = _knob_fact(_route(client, schema))
        assert default_fact != f"knob:{KNOB}=0.400000"

        # Promote the base revision, then the challenger that carries the knob.
        record_id = _promote(client, schema, first["content_digest"])
        moved = _move(
            client,
            {
                "movement": "activate",
                "target": first["content_digest"],
                "receipt_digest": record_id,
            },
        )
        assert moved["recorded"] == "pointer", moved
        assert _knob_fact(_route(client, schema)) == default_fact

        record_id = _promote(client, schema, second["content_digest"])
        moved = _move(
            client,
            {
                "movement": "activate",
                "target": second["content_digest"],
                "receipt_digest": record_id,
            },
        )
        assert moved["recorded"] == "pointer", moved
        assert _knob_fact(_route(client, schema)) == f"knob:{KNOB}=0.400000"

        # One rollback is one pointer move back to the prior revision.
        rolled = _move(client, {"movement": "rollback"})
        assert rolled["recorded"] == "pointer", rolled
        assert _knob_fact(_route(client, schema)) == default_fact
    finally:
        client.close()
