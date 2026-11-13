"""Trajectory decisions of a task plan over a real authenticated loopback engine."""

import time
import uuid

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT

from decide.test_task_plan_served import (  # noqa: F401  (fixture re-export)
    _proposal,
    authenticated_plan_client,
)
from medulla.decision._digest import digest_text
from medulla.decision_client import param, task_plan_request

pytestmark = pytest.mark.no_engine  # Each test owns its actual loopback-TCP engine.

_SCORER_DOMAIN = "eg/trajectory-scorer/v1"


def _scorer(weight: int) -> dict:
    weights = [["loc", weight]]
    return {
        "id": "loc-scorer",
        "revision": 1,
        "digest": digest_text(_SCORER_DOMAIN, ["loc-scorer", 1, weights]),
        "weights": [{"feature_id": "loc", "weight": weight}],
    }


def _budget(value: int, requested_k=None) -> dict:
    query = {
        "decision": "budget",
        "task_digest": "sha256:" + "1" * 64,
        "features": [
            {
                "id": "loc",
                "value": value,
                "unit": "lines",
                "source_ref": "repo:r",
                "source_revision": 3,
                "source_digest": "sha256:" + "2" * 64,
                "observed_at_ms": int(time.time() * 1000),
            }
        ],
        "scorer": _scorer(1_000_000),
    }
    if requested_k is not None:
        query["requested_k"] = requested_k
    return query


def _judged(a: str, b: str, verdict: str) -> dict:
    return {
        "candidate_a": a,
        "digest_a": "sha256:" + a * 64,
        "candidate_b": b,
        "digest_b": "sha256:" + b * 64,
        "judge_revision": "judge-1",
        "verdict": verdict,
        "evidence_digest": "sha256:" + (a + b) * 32,
    }


def _pairwise(records: list, pinned: bool = True) -> dict:
    query = {
        "decision": "pairwise",
        "candidates": [
            {"id": c, "digest": "sha256:" + c * 64} for c in ("a", "b", "c")
        ],
        "records": records,
    }
    if pinned:
        query["policy"] = {"id": "pairwise_copeland", "revision": 1}
    return query


def _ask(pin, query):
    asked = task_plan_request(TEST_TENANT, "fixture-trajectory", _proposal(), pin)
    asked["params"].append(param("trajectory", "trajectory", query))
    return asked


# spec: MEDULLA-DECISION-ENGINE-R140.104
def test_real_decide_route_emits_pinned_refinement_decisions(
    authenticated_plan_client,  # noqa: F811  (the re-exported fixture)
):
    client = authenticated_plan_client
    try:
        pin = publish(
            client,
            "trajectory-schema-" + uuid.uuid4().hex,
            "feature_schema",
            schema_body(),
        )
        plain = client._send(
            "Decide",
            {
                "request": task_plan_request(
                    TEST_TENANT, "fixture-trajectory", _proposal(), pin
                )
            },
        )["records"][0]
        plain_plan = plain["answer"]["value"]
        assert "trajectory" not in plain_plan

        # A complex branch: the answer rides on the plan, bound to the
        # decision, the pinned policy and the source digests.
        record = client._send("Decide", {"request": _ask(pin, _budget(2_000_000))})[
            "records"
        ][0]
        plan = record["answer"]["value"]
        decision = plan["trajectory"]
        answer = decision["answer"]
        assert answer["decision"] == "budget"
        assert (answer["branch"], answer["k_min"], answer["k_max"], answer["k"]) == (
            "complex",
            2,
            8,
            8,
        )
        assert decision["decision_digest"] == answer["decision_digest"]
        assert decision["policy_digest"] == record["inputs"]["policy_digest"]
        assert decision["source_digests"] == sorted(
            [_scorer(1_000_000)["digest"], "sha256:" + "2" * 64]
        )
        assert plan["plan_digest"] == plain_plan["plan_digest"]
        assert record["resolution_kind"] == "constraint"
        assert record["evidence_class"] == "claim"
        subjects = {p["subject"] for p in record["premises"]}
        assert {
            "trajectory:task",
            "trajectory:scorer",
            "trajectory:feature:loc",
            "trajectory:score",
        } <= subjects

        # A full round-robin is ranked by wins minus losses.
        ranked = client._send(
            "Decide",
            {
                "request": _ask(
                    pin,
                    _pairwise(
                        [
                            _judged("a", "b", "win_a"),
                            _judged("a", "c", "win_a"),
                            _judged("b", "c", "win_b"),
                        ]
                    ),
                )
            },
        )["records"][0]["answer"]["value"]["trajectory"]
        assert [r["id"] for r in ranked["answer"]["ranked"]] == ["a", "c", "b"]
        assert ranked["decision_digest"] == ranked["answer"]["decision_digest"]

        # A first attempt starts from the knob.
        first = client._send(
            "Decide",
            {
                "request": _ask(
                    pin,
                    {
                        "decision": "breadth",
                        "search_id": "search",
                        "branch_id": "branch",
                        "ordinal": 1,
                        "plan_digest": "sha256:" + "3" * 64,
                        "history": {"history": "absent"},
                    },
                )
            },
        )["records"][0]["answer"]["value"]["trajectory"]
        assert first["answer"]["selected_breadth"] == 2

        # Refusals leave with their registered codes.
        with pytest.raises(Exception, match="TRAJECTORY_K_INVALID"):
            client._send(
                "Decide", {"request": _ask(pin, _budget(2_000_000, requested_k=99))}
            )
        with pytest.raises(Exception, match="PAIRWISE_POLICY_UNPINNED"):
            client._send(
                "Decide",
                {
                    "request": _ask(
                        pin,
                        _pairwise(
                            [
                                _judged("a", "b", "win_a"),
                                _judged("a", "c", "win_a"),
                                _judged("b", "c", "win_b"),
                            ],
                            pinned=False,
                        ),
                    )
                },
            )
        with pytest.raises(Exception, match="BREADTH_HISTORY_INCOMPLETE"):
            client._send(
                "Decide",
                {
                    "request": _ask(
                        pin,
                        {
                            "decision": "breadth",
                            "search_id": "search",
                            "branch_id": "branch",
                            "ordinal": 1,
                            "plan_digest": "sha256:" + "3" * 64,
                            "history": {
                                "history": "present",
                                "previous_breadth": 2,
                                "refs": [
                                    {
                                        "branch_id": "never-recorded",
                                        "outcome_revision": 1,
                                        "outcome_digest": "sha256:" + "4" * 64,
                                        "result_class": "success",
                                    }
                                ],
                            },
                        },
                    )
                },
            )
    finally:
        client.close()
