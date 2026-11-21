"""Comparative-analysis plan through the served ``Decide`` operation
(MEDULLA-DECISION-ENGINE-R139.6).

The client drives the authenticated loopback engine: library tools are
published, a plan is requested (zero model calls in discovery), then a stale
candidate revision and an over-budget ceiling are asked and refused with the
codes of R139.4.
"""

from __future__ import annotations

from typing import Any

import pytest
from _decision_resident_fixture import publication, publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError

DIGEST = "sha256:" + "1" * 64
TOOLS = ("cand-a", "cand-b", "cand-c")
IMPORTANCE_TENTHS = {"cand-a": 9, "cand-b": 6, "cand-c": 1}


def _tenths(tenths: int) -> dict[str, Any]:
    return {"scale": "q32", "value": (tenths * ds.Q32_ONE) // 10}


def _revisions(client: Any) -> dict[str, int]:
    revisions = {}
    for tool in TOOLS:
        entry = client._send("AgentComponent", publication(tool, "tool", {}))[
            "component"
        ]
        revisions[tool] = entry["entry_revision"]
    return revisions


def _request(
    schema: dict[str, Any], revisions: dict[str, int], ceiling: int
) -> dict[str, Any]:
    candidates = [
        {
            "candidate_id": tool,
            "candidate_revision": revisions[tool],
            "score": _tenths(5),
            "importance": _tenths(IMPORTANCE_TENTHS[tool]),
        }
        for tool in TOOLS
    ]
    body = {
        "space_id": "space:a",
        "space_revision": 3,
        "query_vector_digest": DIGEST,
        "unified_query_result_digest": DIGEST,
        "candidates": candidates,
        "total_call_ceiling": ceiling,
    }
    return dc.decide_request(
        TEST_TENANT,
        ("comparative.test", "comparative_analysis_plan", "ordinary"),
        dc.library(["tool"]),
        schema,
        params=[dc.param("comparative_analysis", "comparative_analysis", body)],
    )


def _decide(client: Any, request: dict[str, Any]) -> dict[str, Any]:
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R139.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R139.6")
def test_plan_and_refusal_through_served_decide(clean_graph):
    client = clean_graph
    schema = publish(
        client,
        "schema-route",
        "feature_schema",
        ds.feature_schema_body([ds.feature("signal", ds.number("signal"))]),
    )
    revisions = _revisions(client)

    record = _decide(client, _request(schema, revisions, 10))
    assert record["resolution_kind"] == "constraint"
    assert record["evidence_class"] == "claim"
    plan = record["answer"]["value"]
    assert plan["total_call_ceiling"] == 10
    assert [c["candidate_id"] for c in plan["candidates"]] == list(TOOLS)
    budgets = {
        budget["stage_id"]: budget["max_calls"] for budget in plan["stage_budgets"]
    }
    # No model call in discovery; synthesis takes importance >= 0.5, extraction
    # takes importance >= 0.8 under the default thresholds.
    assert budgets == {"discovery": 0, "synthesis": 1, "extraction": 1}

    stale = dict(revisions, **{"cand-a": revisions["cand-a"] + 1})
    with pytest.raises(EngineResponseError, match="COMPARATIVE_STALE_INPUT"):
        _decide(client, _request(schema, stale, 10))
    with pytest.raises(EngineResponseError, match="COMPARATIVE_BUDGET_INVALID"):
        _decide(client, _request(schema, revisions, 131))
