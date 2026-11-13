"""MEDULLA-DECISION-ENGINE-R147.6 served distilled ranking: teacher scores for
20 documents are uploaded through `DecisionLog` `Learn`; `Decide` then ranks 10
visible graph candidates from the stored scores alone (`top_k` 3) and seals the
scorer evidence; another tenant is refused."""

from __future__ import annotations

from typing import Any

import pytest
from _decision_resident_fixture import publish
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError

DOCS = 20
VISIBLE = 10
SCORER = "scorer-served"
LOW_FIDELITY_Q16 = 40_000
HIGH_Q16 = 65_536


def _score(index: int) -> int:
    # 7 is coprime with 20, so the twenty scores are distinct.
    return 100 * ((index * 7) % DOCS) + 1


def _upload(client: Any, revision: int) -> dict[str, Any]:
    scores = [
        {
            "document_id": f"doc-{i:02}",
            "score_q16": _score(i),
            "fidelity_lo_q16": LOW_FIDELITY_Q16,
            "fidelity_hi_q16": HIGH_Q16,
        }
        for i in range(DOCS)
    ]
    op = dc.learn_op(
        TEST_TENANT,
        "upload_distilled_scores",
        upload={
            "scorer_id": SCORER,
            "scorer_revision": revision,
            "scores": scores,
        },
    )
    result = client._send("DecisionLog", {"op": op})
    # The manifest is the recorded value; the wire may nest it one level down.
    if "score_count" in result:
        return result
    return next(
        v for v in result.values() if isinstance(v, dict) and "score_count" in v
    )


def _rank_request(
    client: Any, schema: dict[str, Any], tenant: str, top_k: int
) -> dict[str, Any]:
    graph = client._client._graph_name
    request = dc.decide_request(
        tenant,
        ("rank.distilled", "rank", "ordinary"),
        {
            "source": "graph",
            "graph": graph,
            "plan": {"ops": [{"Scan": {"label": "Tool"}}]},
        },
        schema,
        params=[
            dc.param(
                "distilled_scorer",
                "distilled_scorer",
                {"scorer_id": SCORER, "scorer_revision": 1, "top_k": top_k},
            )
        ],
        max_records=1,
    )
    request["belief_as_of"] = []
    return request


# spec: MEDULLA-DECISION-ENGINE-R147.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R147.6")
def test_index_time_scoring_and_query_ranking(clean_graph):
    for i in range(DOCS):
        # Only the first ten documents are Tool nodes the ranking can see.
        kind = "Tool" if i < VISIBLE else "Archived"
        clean_graph.nodes.add(f"doc-{i:02}", {"type": kind, "score": 0.5})
    body = ds.feature_schema_body([ds.feature("score", ds.number("score"))])
    schema = publish(clean_graph, "distilled-schema", "feature_schema", body)

    manifest = _upload(clean_graph, 1)
    assert manifest["scorer_id"] == SCORER and manifest["score_count"] == DOCS, manifest
    # Replaying the same batch is idempotent: the manifest does not change.
    assert _upload(clean_graph, 1) == manifest

    record = clean_graph._send(
        "Decide", {"request": _rank_request(clean_graph, schema, TEST_TENANT, 3)}
    )["records"][0]
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    expected = sorted(range(VISIBLE), key=lambda i: -_score(i))[:3]
    assert [s["option_id"] for s in outcome["scores"]] == [
        f"doc-{i:02}" for i in expected
    ]
    facts = {premise["fact"] for premise in record["premises"]}
    assert "distilled:model_calls=0" in facts
    assert f"distilled:scorer={SCORER}@1" in facts
    assert f"distilled:score_count={DOCS}" in facts

    # Another tenant is refused: it has no scores and no access to this graph.
    with pytest.raises(EngineResponseError):
        clean_graph._send(
            "Decide",
            {"request": _rank_request(clean_graph, schema, "other-tenant", 3)},
        )
