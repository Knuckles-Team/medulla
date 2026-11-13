"""Search-task synthesis over the served wire (MEDULLA-FEDERATED-QUERY-R702.3).

The session fixture in conftest starts the native server. The journey stores two
evidence rings, synthesizes one task around each answer node through the generated
client method, turns the accepted tasks into the pinned query set and label set of
a retrieval ranking evaluation and submits it to ``DecisionEval``.
"""

from __future__ import annotations

import hashlib

import pytest
from conftest import TEST_TENANT

from medulla.client import EngineResponseError

# Each ring is the answer node joined to three evidence nodes in a cycle, so no
# clue decides the answer alone and no clue's evidence sits beside many others.
_RINGS = {
    "ent-orion": ("Orion Labs", ["Harbor Works", "Delta Mill", "Kestrel Yard"]),
    "ent-vega": ("Vega Foundry", ["Quarry Hall", "Lantern Dock", "Willow Press"]),
}

_K = 1
_EVALUATOR_REVISION = 3


def _seed(gc) -> None:
    for answer, (name, members) in _RINGS.items():
        gc.nodes.add(answer, {"type": "Company", "name": name, "text": name})
        ids = [f"{answer}-m{n}" for n in range(len(members))]
        for node_id, member in zip(ids, members, strict=True):
            gc.nodes.add(node_id, {"type": "Site", "name": member, "text": member})
        chain = [answer, *ids, answer]
        for source, target in zip(chain, chain[1:], strict=False):
            gc.edges.add(source, target, {"relationship": "SUPPLIES"})


def _digest(domain: str, lines: list[str]) -> str:
    hasher = hashlib.sha256()
    hasher.update(domain.encode())
    hasher.update(b"\x00")
    for line in lines:
        hasher.update(line.encode())
        hasher.update(b"\n")
    return hasher.hexdigest()


def _query_id(task: dict) -> str:
    return "task:" + hashlib.sha256(task["question"].encode()).hexdigest()[:16]


def _ranking_request(accepted: dict[str, dict]) -> dict:
    """One ranking evaluation whose query set is the accepted tasks."""
    query_ids = sorted(_query_id(result["task"]) for result in accepted.values())
    pairs = sorted(
        (_query_id(result["task"]), result["task"]["answer_node"])
        for result in accepted.values()
    )
    labels_digest = _digest("eg/ranking-labels/v1", [f"{q}\t{d}" for q, d in pairs])
    queries_digest = _digest("eg/ranking-queries/v1", query_ids)
    corpus_digest = hashlib.sha256(b"search-task-corpus").hexdigest()
    input_digest = _digest(
        "eg/ranking-input/v1",
        [
            f"corpus {corpus_digest}",
            f"labels {labels_digest}",
            f"queries {queries_digest}",
            f"k {_K}",
            f"evaluator {_EVALUATOR_REVISION}",
        ],
    )

    def run(algorithm_id: str, best_first: dict[str, list[str]]) -> dict:
        return {
            "algorithm_id": algorithm_id,
            "version": "1",
            "settings_digest": hashlib.sha256(algorithm_id.encode()).hexdigest(),
            "corpus_digest": corpus_digest,
            "input_digest": input_digest,
            "rankings": [
                {"query_id": query_id, "ranked_ids": ids}
                for query_id, ids in sorted(best_first.items())
            ],
        }

    orion, vega = (accepted[node]["task"] for node in ("ent-orion", "ent-vega"))
    return {
        "op": {
            "op": "retrieval_ranking_submit",
            "request": {
                "schema_version": 1,
                "tenant_id": TEST_TENANT,
                "idempotency_key": "search-task-feeds-ranking",
                "corpus": {"id": "corpus", "revision": 1, "digest": corpus_digest},
                "labels": {
                    "pinned": {"id": "labels", "revision": 1, "digest": labels_digest},
                    "pairs": [{"query_id": q, "doc_id": d} for q, d in pairs],
                },
                "queries": {
                    "pinned": {
                        "id": "queries",
                        "revision": 1,
                        "digest": queries_digest,
                    },
                    "query_ids": query_ids,
                },
                "k": _K,
                "evaluator_revision": _EVALUATOR_REVISION,
                "algorithms": [
                    # The walk over the evidence ranks both answers first.
                    run(
                        "evidence-walk",
                        {
                            _query_id(orion): [orion["answer_node"], "other"],
                            _query_id(vega): [vega["answer_node"], "other"],
                        },
                    ),
                    # The keyword baseline finds one of the two.
                    run(
                        "keyword",
                        {
                            _query_id(orion): ["other", orion["answer_node"]],
                            _query_id(vega): [vega["answer_node"], "other"],
                        },
                    ),
                ],
            },
        }
    }


# spec: MEDULLA-FEDERATED-QUERY-R702.3
def test_task_feeds_ranking_evaluation(clean_graph):
    gc = clean_graph
    _seed(gc)

    accepted = {
        answer: gc.graph.synthesize_search_task(
            tenant_id=TEST_TENANT, answer_node=answer
        )
        for answer in _RINGS
    }
    for answer, result in accepted.items():
        task = result["task"]
        assert task["answer_node"] == answer
        assert len(task["clues"]) == len(_RINGS[answer][1])
        assert task["question"].startswith("Which ")
        assert [f["detector"] for f in result["report"]["findings"]] == [
            "single_clue",
            "co_coverage",
            "exposed_constant",
            "prior_knowledge",
        ]
        # An accepted task is within every threshold it was judged against.
        for finding in result["report"]["findings"]:
            assert finding["value"] <= result["thresholds"][finding["detector"]]
        assert result["evidence_nodes"] == 1 + len(_RINGS[answer][1])
    assert _query_id(accepted["ent-orion"]["task"]) != _query_id(
        accepted["ent-vega"]["task"]
    )

    # The engine wrote nothing: the same call answers the same task.
    again = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-orion"
    )
    assert again == accepted["ent-orion"]

    # The caller sends the accepted tasks as the pinned query set of one ranking
    # evaluation, and the sealed receipt scores the two algorithms exactly.
    job = gc._send("DecisionEval", _ranking_request(accepted))
    assert job["state"]["state"] == "succeeded"
    receipt = job["state"]["output"]["receipt"]
    scores = {score["algorithm_id"]: score for score in receipt["scores"]}
    assert (
        scores["evidence-walk"]["numerator"],
        scores["evidence-walk"]["denominator"],
    ) == (2, 2)
    assert scores["evidence-walk"]["value_ppm"] == 1_000_000
    assert (scores["keyword"]["numerator"], scores["keyword"]["denominator"]) == (1, 2)
    assert scores["keyword"]["value_ppm"] == 500_000
    assert receipt["k"] == _K

    # A request for another tenant and an answer node that is not stored are
    # refused with typed codes.
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
        gc.graph.synthesize_search_task(
            tenant_id="tenant:other", answer_node="ent-orion"
        )
    with pytest.raises(EngineResponseError, match="SEARCH_TASK_INVALID"):
        gc.graph.synthesize_search_task(tenant_id=TEST_TENANT, answer_node="ent-absent")
