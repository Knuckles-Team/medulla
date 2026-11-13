"""MEDULLA-DECISION-ENGINE-R614.3 served `SemanticEnrich`: the four scheduled
enrichment ops over the wire, and the refusal of an unscheduled call.

Every op names the committed `EnrichmentSchedule` decision that chose its work
id `semantic:<op>:<subject>`. The schedule decisions here are real: each is
the served `Decide` answer of an `enrichment_schedule` question under a pinned
policy that permits exploration for that question, committed through
`DecisionLog`. The graph evidence (a fact, a symbol, a Document, two topics
with centroid vectors, and articles that carry a topic reference) is written
through the generated client write and `AddEmbedding`.
"""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import publication, publish
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op as _op
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError, MedullaClient
from medulla.decision import policy_digest

Q32 = 1 << 32
QUESTION = "q:semantic-enrich"
TAXONOMY = "subjects"
EXPECTED_VALUE_KEY = "enrichment.expected_value_q32"
COST_KEY = "enrichment.cost_q32"


def _policy_body() -> dict[str, Any]:
    """The engine default policy, except that exploration is permitted for the
    scheduling question: with one candidate the explored option is that one."""
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


def _publish_policy(client: Any) -> dict[str, Any]:
    body = _policy_body()
    request = publication("enrich-policy", "decision_policy", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = policy_digest(body)
    component["attributes"] = {"decision.policy": json.dumps(body)}
    entry = client._send("AgentComponent", request)["component"]
    return {
        "component_id": "enrich-policy",
        "kind": "decision_policy",
        "definition_digest": entry["definition_digest"],
    }


def _schema_body() -> dict[str, Any]:
    return ds.feature_schema_body(
        [
            ds.feature("cost", ds.number(COST_KEY)),
            ds.feature("expected_value", ds.number(EXPECTED_VALUE_KEY)),
        ]
    )


async def _schedule(
    schema: dict[str, Any], policy: dict[str, Any], work_id: str
) -> str:
    """Decide the schedule of one work id and commit the decision."""
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        option = dc.DeclaredOption(
            work_id, numbers={EXPECTED_VALUE_KEY: 10 * Q32, COST_KEY: Q32}
        )
        request = dc.decide_request(
            TEST_TENANT,
            (QUESTION, "enrichment_schedule", "ordinary"),
            dc.declared([option]),
            schema,
            policy={"policy": "pinned", "component": policy},
        )
        payload = await decision.decide(request)
        record = payload["records"][0]
        assert record["outcome"]["outcome"] in ("explored", "acted"), record["outcome"]
        assert record["outcome"]["option_id"] == work_id
        await decision.log(dc.commit_op(record))
        return record["record_id"]
    finally:
        await client.close()


def _seed(clean_graph: Any) -> None:
    nodes: list[tuple[str, dict[str, Any]]] = [
        ("fact:1", {"type": "Fact", "text": "main parses the request"}),
        ("fact:2", {"type": "Fact", "text": "main writes the store"}),
        ("sym:main", {"type": "Symbol", "name": "main"}),
        ("doc:1", {"type": "Document", "text": "parsing and storing requests"}),
        ("topic:a", {"type": "Topic", "taxonomy": TAXONOMY, "name": "parsing"}),
        ("topic:b", {"type": "Topic", "taxonomy": TAXONOMY, "name": "weather"}),
        ("art:1", {"type": "Article", "category": "topic:a"}),
        ("art:2", {"type": "Article", "category": "topic:missing"}),
    ]
    envelope = _envelope(
        _graph_name(clean_graph),
        "w271:seed",
        "w271:seed",
        1,
        [
            _op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ],
    )
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results
    for node_id, vector in [
        ("doc:1", [1.0, 0.0, 0.0, 0.0]),
        ("topic:a", [1.0, 0.1, 0.0, 0.0]),
        ("topic:b", [0.0, 0.0, 1.0, 0.0]),
    ]:
        clean_graph._send("AddEmbedding", {"node_id": node_id, "embedding": vector})


def _call(client: Any, op: dict[str, Any]) -> dict[str, Any]:
    return client._send("SemanticEnrich", {"op": op})


def _counts(client: Any) -> tuple[int, int]:
    return client.nodes.count(), client._send("EdgeCount")


# spec: MEDULLA-DECISION-ENGINE-R614.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R614.3")
def test_card_rank_classify_project_journey(clean_graph):
    _seed(clean_graph)
    schema = publish(clean_graph, "enrich-schema", "feature_schema", _schema_body())
    policy = _publish_policy(clean_graph)

    def scheduled(op_name: str, subject: str) -> str:
        return asyncio.run(_schedule(schema, policy, f"semantic:{op_name}:{subject}"))

    # An unscheduled call refuses and writes nothing: no such decision, and a
    # decision that chose other work.
    before = _counts(clean_graph)
    card = {
        "op": "card",
        "subject": "sym:main",
        "summary": "main wires the parser to the store",
        "model": "model:text-7b",
        "prompt_digest": "ab" * 32,
        "source_fact_ids": ["fact:1", "fact:2"],
    }
    with pytest.raises(EngineResponseError, match="ENRICHMENT_NOT_SCHEDULED"):
        _call(clean_graph, {**card, "schedule_record_id": "record:absent"})
    other = scheduled("card", "sym:other")
    with pytest.raises(EngineResponseError, match="ENRICHMENT_NOT_SCHEDULED"):
        _call(clean_graph, {**card, "schedule_record_id": other})
    assert _counts(clean_graph) == before

    # Card: one node and the hasCard edge from its subject.
    receipt = _call(
        clean_graph, {**card, "schedule_record_id": scheduled("card", "sym:main")}
    )
    assert receipt["op"] == "card" and len(receipt["node_ids"]) == 1
    assert receipt["edge_count"] == 1
    assert _counts(clean_graph) == (before[0] + 1, before[1] + 1)

    # RankProposals: the best concepts first, one node each.
    after_card = _counts(clean_graph)
    ranked = _call(
        clean_graph,
        {
            "op": "rank_proposals",
            "schedule_record_id": scheduled("rank_proposals", "sym:main"),
            "subject": "sym:main",
            "concepts": [
                {
                    "concept": "c:cache",
                    "impact_bp": 9000,
                    "confidence_bp": 8000,
                    "cost_units": 2,
                },
                {
                    "concept": "c:index",
                    "impact_bp": 1000,
                    "confidence_bp": 5000,
                    "cost_units": 4,
                },
            ],
        },
    )
    assert len(ranked["node_ids"]) == 2
    assert _counts(clean_graph) == (after_card[0] + 2, after_card[1])

    # Classify: the document joins the nearest topic, with its confidence.
    after_rank = _counts(clean_graph)
    classified = _call(
        clean_graph,
        {
            "op": "classify",
            "schedule_record_id": scheduled("classify", "doc:1"),
            "subject": "doc:1",
            "taxonomy": TAXONOMY,
        },
    )
    assert classified["topic"] == "topic:a"
    assert classified["confidence_bp"] >= 9000
    assert _counts(clean_graph) == (after_rank[0] + 1, after_rank[1] + 1)

    # Project: the resolvable reference becomes an edge, the other is counted.
    after_classify = _counts(clean_graph)
    projected = _call(
        clean_graph,
        {
            "op": "project",
            "schedule_record_id": scheduled("project", "Article"),
            "subject": "Article",
            "rules": [
                {
                    "id": "rule:category",
                    "taxonomy": TAXONOMY,
                    "property": "category",
                    "relation": "inCategory",
                }
            ],
        },
    )
    assert projected["edge_count"] == 1 and projected["unresolved"] == 1
    assert projected["more"] is False
    assert _counts(clean_graph) == (after_classify[0], after_classify[1] + 1)
