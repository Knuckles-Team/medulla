"""``ContextFacts`` served after ``Discover`` (MEDULLA-DECISION-ENGINE-R152.6).

The journey: rank nodes with ``discover``, pass the ranked ids to ``context_facts``
and read the cited bundle. Runs against the shared native engine through the
``clean_graph`` fixture.
"""

import pytest

from medulla.client import EngineResponseError


def _seed(gc):
    gc.nodes.add(
        "ctx-deploy",
        {
            "type": "Runbook",
            "name": "Deploy Runbook",
            "description": "how to deploy a service",
            "source_key": "wiki/deploy",
            "evidence_class": "observation",
        },
    )
    gc.nodes.add(
        "ctx-rollback",
        {
            "type": "Runbook",
            "name": "Rollback Runbook",
            "description": "how to roll back a deploy",
            "source_key": "wiki/rollback",
            "evidence_class": "proof",
        },
    )


# spec: MEDULLA-DECISION-ENGINE-R152.6
def test_context_facts_after_discover(clean_graph):
    gc = clean_graph
    _seed(gc)

    ranked = gc.graph.discover(["deploy", "runbook"], [], k=5)
    ids = [row["id"] for row in ranked if row["id"].startswith("ctx-")]
    assert ids, "discover must rank the seeded nodes"

    bundle = gc.graph.context_facts(ids, token_budget=10_000)
    assert bundle["omitted_count"] == 0
    assert bundle["facts"], "ranked nodes yield facts"
    for fact in bundle["facts"]:
        assert fact["source_key"].startswith("wiki/")
        assert len(fact["fact_id"]) == 64
        assert fact["authority"] in {"proof", "observation", "claim"}

    tight = gc.graph.context_facts(ids, token_budget=bundle["token_estimate_total"] - 1)
    assert tight["token_estimate_total"] <= bundle["token_estimate_total"] - 1
    assert tight["omitted_count"] >= 1

    with pytest.raises(EngineResponseError, match="CONTEXT_FACTS_BUDGET_ZERO"):
        gc.graph.context_facts(ids, token_budget=0)
