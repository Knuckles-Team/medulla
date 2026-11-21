"""``IndexlessRetrieve`` served journey (MEDULLA-DECISION-ENGINE-R144.6, R144.7).

The journey: store 200 ``Document`` nodes, scan them through the generated client
method under ``max_rows`` 100 for 50 rows, then for 101 rows, read the cost report
and the refusal, and watch an over-ceiling budget clamp to the tenant ceiling.
Runs against the shared native engine through the ``clean_graph`` fixture.
"""

import json

import pytest
from conftest import TEST_TENANT

from medulla.client import EngineResponseError

_CORPUS = 200


def _seed(gc):
    for n in range(_CORPUS):
        gc.nodes.add(f"il-{n:03}", {"type": "Document", "text": f"document {n}"})


def _cost(error):
    """The cost report a refusal detail carries."""
    _, _, report = (error.detail or "").partition("; cost ")
    return json.loads(report)


# spec: MEDULLA-DECISION-ENGINE-R144.6
def test_budget_enforcement_and_cost_reporting(clean_graph):
    gc = clean_graph
    _seed(gc)
    budget = {"max_rows": 100, "max_visits": 1000, "max_cost_units": 1000}

    result = gc.graph.indexless_retrieve(
        tenant_id=TEST_TENANT,
        ops=[{"op": "corpus_scan", "limit": 50}],
        budget=budget,
    )
    assert [row["document_id"] for row in result["rows"]] == [
        f"il-{n:03}" for n in range(50)
    ]
    cost = result["cost"]
    assert cost["rows_visited"] == 50
    assert cost["cost_used"] == 50
    assert cost["remaining"]["max_rows"] == 50
    assert cost["applied_budget"] == budget
    assert cost["operation_ids"] == ["0:corpus_scan"]
    assert cost["authorization_revision"] > 0

    with pytest.raises(EngineResponseError) as refused:
        gc.graph.indexless_retrieve(
            tenant_id=TEST_TENANT,
            ops=[{"op": "corpus_scan", "limit": 101}],
            budget=budget,
        )
    assert refused.value.code == "INDEXLESS_BUDGET_EXHAUSTED"
    report = _cost(refused.value)
    assert report["rows_visited"] == 0
    assert report["remaining"]["max_rows"] == 100
    assert report["operation_ids"] == []


# spec: MEDULLA-DECISION-ENGINE-R144.7
def test_consumer_budget_clamped_to_tenant_ceiling(clean_graph):
    gc = clean_graph
    _seed(gc)
    ceiling = {"max_rows": 10000, "max_visits": 100000, "max_cost_units": 1000000}

    clamped = gc.graph.indexless_retrieve(
        tenant_id=TEST_TENANT,
        ops=[{"op": "corpus_scan", "limit": 5}],
        budget={"max_rows": 20000, "max_visits": 50, "max_cost_units": 2000000},
    )
    assert clamped["cost"]["applied_budget"] == {
        "max_rows": 10000,
        "max_visits": 50,
        "max_cost_units": 1000000,
    }
    assert clamped["cost"]["premises"] == [
        "knob:indexless.max_rows=10000",
        "knob:indexless.max_visits=100000",
        "knob:indexless.max_cost_units=1000000",
    ]

    open_budget = gc.graph.indexless_retrieve(
        tenant_id=TEST_TENANT, ops=[{"op": "corpus_scan", "limit": 5}]
    )
    assert open_budget["cost"]["applied_budget"] == ceiling

    # Cross-tenant negative: another tenant's name is refused before any read.
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
        gc.graph.indexless_retrieve(
            tenant_id="tenant:other", ops=[{"op": "corpus_scan", "limit": 5}]
        )
