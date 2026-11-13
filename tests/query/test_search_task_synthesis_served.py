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
from medulla.generated.models import SearchTaskRejection, ShortcutDetector

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


# spec: MEDULLA-FEDERATED-QUERY-R702.5
def test_risk_report_lists_values(clean_graph):
    gc = clean_graph
    _seed(gc)
    # A star: every leaf is decided by the answer alone, so single_clue is 1.0.
    gc.nodes.add("ent-star", {"type": "Company", "name": "Star Co", "text": "Star Co"})
    for n in range(3):
        leaf = f"ent-star-l{n}"
        gc.nodes.add(leaf, {"type": "Site", "name": f"Leaf {n}", "text": f"Leaf {n}"})
        gc.edges.add("ent-star", leaf, {"relationship": "SUPPLIES"})

    detectors = ["single_clue", "co_coverage", "exposed_constant", "prior_knowledge"]
    accepted = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-orion"
    )
    report = accepted["report"]
    assert [f["detector"] for f in report["findings"]] == detectors
    for finding in report["findings"]:
        assert isinstance(finding["value"], float)
        assert isinstance(finding["nodes"], list)
        assert finding["value"] <= accepted["thresholds"][finding["detector"]]
    # The prior-knowledge finding names the answer node itself.
    assert report["findings"][3]["nodes"] == ["ent-orion"]

    rejected = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-star"
    )
    # A typed rejection, not an error: the code, the tripped detector and the
    # complete report arrive as values.
    assert isinstance(rejected, SearchTaskRejection)
    assert rejected.code == "SEARCH_TASK_SHORTCUT"
    assert rejected.detector == ShortcutDetector.SINGLE_CLUE
    assert [f.detector.value for f in rejected.report.findings] == detectors
    single = rejected.report.findings[0]
    assert single.value == 1.0
    assert sorted(single.nodes) == [f"ent-star-l{n}" for n in range(3)]
    assert rejected.report.findings[3].nodes == ["ent-star"]
    assert rejected.report.findings[3].value == 3.0
    assert single.value > rejected.thresholds.single_clue


def _seed_star(gc) -> list[str]:
    """One answer node whose three leaves hang off it alone: a single-clue chain."""
    gc.nodes.add("ent-star", {"type": "Company", "name": "Star Co", "text": "Star Co"})
    leaves = [f"ent-star-l{n}" for n in range(3)]
    for n, leaf in enumerate(leaves):
        gc.nodes.add(leaf, {"type": "Site", "name": f"Leaf {n}", "text": f"Leaf {n}"})
        gc.edges.add("ent-star", leaf, {"relationship": "SUPPLIES"})
    return leaves


# spec: MEDULLA-FEDERATED-QUERY-R702.4
def test_risky_task_refused_with_detector_name(clean_graph):
    gc = clean_graph
    _seed(gc)
    _seed_star(gc)
    nodes_before, edges_before = gc.nodes.count(), gc.edges.count()

    # A task over the single-clue threshold is refused, naming the detector.
    rejected = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-star"
    )
    assert isinstance(rejected, SearchTaskRejection)
    assert rejected.code == "SEARCH_TASK_SHORTCUT"
    assert rejected.detector == ShortcutDetector.SINGLE_CLUE
    tripped = next(
        f for f in rejected.report.findings if f.detector == rejected.detector
    )
    assert tripped.value > rejected.thresholds.single_clue

    # A task within every threshold is not refused.
    accepted = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-orion"
    )
    assert not isinstance(accepted, SearchTaskRejection)

    # Every other refusal is a registered code, and none of them writes.
    with pytest.raises(EngineResponseError, match="SEARCH_TASK_INVALID"):
        gc.graph.synthesize_search_task(tenant_id=TEST_TENANT, answer_node="ent-absent")
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
        gc.graph.synthesize_search_task(
            tenant_id="tenant:other", answer_node="ent-star"
        )
    assert (gc.nodes.count(), gc.edges.count()) == (nodes_before, edges_before)


# spec: MEDULLA-FEDERATED-QUERY-R702.6
def test_single_clue_task_is_rejected(clean_graph):
    gc = clean_graph
    # A 200-node store: one single-clue star (4 nodes) among 49 four-node rings.
    leaves = _seed_star(gc)
    for ring in range(49):
        answer = f"ent-noise-{ring}"
        gc.nodes.add(
            answer,
            {"type": "Company", "name": f"Noise {ring}", "text": f"Noise {ring}"},
        )
        ids = [f"{answer}-m{n}" for n in range(3)]
        for n, node_id in enumerate(ids):
            name = f"Member {ring} {n}"
            gc.nodes.add(node_id, {"type": "Site", "name": name, "text": name})
        chain = [answer, *ids, answer]
        for source, target in zip(chain, chain[1:], strict=False):
            gc.edges.add(source, target, {"relationship": "SUPPLIES"})
    assert gc.nodes.count() == 200

    rejected = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-star"
    )
    assert isinstance(rejected, SearchTaskRejection)
    assert rejected.code == "SEARCH_TASK_SHORTCUT"
    assert rejected.detector == ShortcutDetector.SINGLE_CLUE
    single = rejected.report.findings[0]
    assert single.detector == ShortcutDetector.SINGLE_CLUE
    assert single.value == 1.0
    assert sorted(single.nodes) == sorted(leaves)
    # The 196 other nodes are outside the star's neighbourhood: the task is judged on
    # the evidence around its own answer node only, and a ring in the same store passes.
    accepted = gc.graph.synthesize_search_task(
        tenant_id=TEST_TENANT, answer_node="ent-noise-0"
    )
    assert not isinstance(accepted, SearchTaskRejection)
    assert accepted["task"]["answer_node"] == "ent-noise-0"
