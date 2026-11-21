"""PROPAGATE ... RISK HISTORY over the served wire (MEDULLA-FEDERATED-QUERY-R032.100).

Failure history is ``:Observation`` facts on the pinned snapshot. Three layers, differing
failure counts, a dependency cycle and a node with no observations cover both directions,
the path formulas, a null aggregate with ``partial``, and the single refusal for an absent
dataset.
"""

from __future__ import annotations

import uuid

import pytest

from medulla.client import EngineResponseError

LAYERS = "LAYERS ('service' 1.0, 'os' 0.5, 'hardware' 0.25)"


def _observe(graph, tag: str, dataset: str, target: str, outcome: str, index: int) -> None:
    observation = f"obs-{tag}-{target}-{index}"
    graph.nodes.add(
        observation,
        {
            "type": "Observation",
            "dataset": dataset,
            "observationTime": 100 + index,
            "outcome": outcome,
        },
    )
    graph.edges.add(observation, target, {"relationship": "observedEntity"})


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R032.100")
def test_historical_risk_both_directions(clean_graph) -> None:
    tag = uuid.uuid4().hex[:8]
    dataset = f"deploys-{tag}"
    a, b, c, d = (f"{name}-{tag}" for name in ("a", "b", "c", "d"))
    for node, layer in ((a, "service"), (b, "os"), (c, "hardware"), (d, "service")):
        clean_graph.nodes.add(node, {"type": f"Svc{tag}", "layer": layer})
    # a -> b -> c -> a is a cycle; d depends on a and has no observations.
    for source, target in ((a, b), (b, c), (c, a), (d, a)):
        clean_graph.edges.add(source, target, {"relationship": "dependsOn"})
    index = 0
    for target, outcomes in ((a, ["failure", "success"]), (b, ["failure"]), (c, ["success"])):
        for outcome in outcomes:
            _observe(clean_graph, tag, dataset, target, outcome, index)
            index += 1

    tail = f"RISK HISTORY '{dataset}' WINDOW 0 TO 1000 {LAYERS}"
    query = (
        f"MATCH (:Svc{tag}) |> WHERE id = '{a}' "
        f"|> PROPAGATE NOISY_OR -[:dependsOn]-> HOPS 4"
    )
    plain = clean_graph.query.uql(f"{query} |> RETURN impact")
    result = clean_graph.query.uql(f"{query} {tail} |> RETURN impact, history_risk")

    assert result["kind"] == "rows"
    assert [row["channels"]["impact"] for row in result["rows"]] == [
        row["channels"]["impact"] for row in plain["rows"]
    ]
    (risk,) = result["historical_risk"]
    assert risk["target"] == a
    downstream = {node["node_id"]: node for node in risk["downstream"]}
    upstream = {node["node_id"]: node for node in risk["upstream"]}
    # a: 1 of 2 at weight 1.0 = 0.5; b: 1 of 1 at 0.5 = 0.5; c: 0 of 1.
    assert downstream[b]["layer_contribution"] == pytest.approx(0.5)
    assert downstream[b]["path_risk"] == pytest.approx(1 - 0.5 * 0.5)
    assert downstream[c]["path"] == [a, b, c]
    assert a not in downstream  # a cycle never repeats a node
    assert upstream[c]["path"] == [a, c]
    # d has no observations: unknown coverage, a null aggregate and partial.
    assert upstream[d]["coverage"] == "unknown"
    assert upstream[d]["path_risk"] is None
    assert risk["partial"] is True
    assert risk["aggregate_risk"] is None

    for text in (
        f"{query} RISK HISTORY 'absent-{tag}' WINDOW 0 TO 1000 {LAYERS}",
        f"{query} RISK HISTORY '{dataset}' WINDOW 5000 TO 6000 {LAYERS}",
    ):
        with pytest.raises(EngineResponseError) as refused:
            clean_graph.query.uql(text)
        assert "HISTORY_DATASET_UNAVAILABLE" in str(refused.value)
