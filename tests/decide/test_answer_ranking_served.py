"""Verifier-free answer ranking over a real authenticated loopback wire."""

import uuid

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT

from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)
from medulla import decision_client as dc

pytestmark = pytest.mark.no_engine  # The fixture owns its loopback-TCP engine.

CANDIDATES = ["d", "c", "b", "a"]


def _pair(first, second, preference):
    return {"first": first, "second": second, "preference": preference}


def _ranking():
    """`c` wins every comparison, `a` beats `b` and ties `d`, `b` ties `d`, and
    `d` loses to `c`: scores c=6, a=3, d=2, b=1."""
    return {
        "candidate_ids": CANDIDATES,
        "comparisons": [
            _pair("a", "b", "first"),
            _pair("a", "c", "second"),
            _pair("a", "d", "tie"),
            _pair("c", "b", "first"),
            _pair("b", "d", "tie"),
            _pair("d", "c", "second"),
        ],
        "verifier": None,
        "retain_top_k": 2,
        "max_revisions": 2,
    }


def _ask(client, pin, question_id):
    return dc.decide_request(
        TEST_TENANT,
        (question_id, "answer_ranking", "ordinary"),
        dc.declared(dc.DeclaredOption(name) for name in CANDIDATES),
        pin,
        params=[dc.param("answer_ranking", "answer_ranking", _ranking())],
        max_records=1,
    )


# spec: MEDULLA-DECISION-ENGINE-R134.109
def test_revisions_only_for_non_retained(authenticated_plan_client):  # noqa: F811
    client = authenticated_plan_client
    try:
        pin = publish(
            client,
            "answer-ranking-schema-" + uuid.uuid4().hex,
            "feature_schema",
            schema_body(),
        )
        asked = _ask(client, pin, "answer-ranking-journey")
        record = client._send("Decide", {"request": asked})["records"][0]
        assert record["question"]["kind"] == "answer_ranking"

        # A deterministic global order, ties broken by the lowest candidate id.
        scores = [
            (s["option_id"], s["score"]["value"] >> 32)
            for s in record["outcome"]["scores"]
        ]
        assert scores == [("c", 6), ("a", 3), ("d", 2), ("b", 1)]
        order = [
            p
            for p in record["premises"]
            if p["fact"].startswith("answer_global_order:")
        ]
        assert len(order) == 1
        assert "order=c,a,d,b" in order[0]["fact"]
        assert "retained=c,a" in order[0]["fact"]

        # Revisions exist only for the two non-retained candidates.
        revised = sorted(
            p["subject"]
            for p in record["premises"]
            if p["fact"].startswith("answer_revision:")
        )
        assert revised == ["b", "d"]
        retained = sorted(
            p["subject"]
            for p in record["premises"]
            if p["fact"].startswith("answer_retained:")
        )
        assert retained == ["a", "c"]

        # Commit, then read the record back by decision id: equal bytes.
        committed = client._send("DecisionLog", {"op": dc.commit_op(record)})
        assert committed["record_digest"] == record["record_digest"]
        entry = client._send(
            "DecisionLog",
            {"op": dc.get_op(TEST_TENANT, committed["record_id"])},
        )
        assert entry["record"] == record

        # The same request seals the same ranking again.
        again = client._send("Decide", {"request": asked})["records"][0]
        assert again["premises"] == record["premises"]
        assert again["outcome"] == record["outcome"]
    finally:
        client.close()
