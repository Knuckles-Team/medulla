"""Topology analytics over the served wire (MEDULLA-FEDERATED-QUERY-R607.3).

The session fixture in conftest starts the native server. The fixture graph is two
triangles joined by one bridge edge, so every expected value is stated by hand from
the published definitions: the global minimum cut is the bridge (weight 1), the
spectral partition into two recovers the triangles, and the two mirrored triangle
nodes of different labels are an exact analogy.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError

_LEFT = ["tri-a1", "tri-a2", "tri-a3"]
_RIGHT = ["tri-b1", "tri-b2", "tri-b3"]
_BRIDGE = ("tri-a3", "tri-b1")


def _seed(gc) -> None:
    for node in _LEFT:
        gc.nodes.add(node, {"type": "Alpha", "name": node})
    for node in _RIGHT:
        gc.nodes.add(node, {"type": "Beta", "name": node})
    for side in (_LEFT, _RIGHT):
        for source, target in ((0, 1), (1, 2), (0, 2)):
            gc.edges.add(side[source], side[target], {"relationship": "LINKS"})
    gc.edges.add(*_BRIDGE, {"relationship": "LINKS"})


# spec: MEDULLA-FEDERATED-QUERY-R607.3
def test_method_is_registered_and_reaches_kernels(clean_graph):
    gc = clean_graph
    _seed(gc)

    mincut = gc.graph.mine_topology("mincut")
    assert mincut["analytic"] == "mincut"
    assert mincut["evidence"]["cut_weight"] == 1.0
    sides = sorted(sorted(p["members"]) for p in mincut["partitions"])
    assert sides == [_LEFT, _RIGHT]

    spectral = gc.graph.mine_topology("spectral", k=2, seed=7)
    assert spectral["analytic"] == "spectral"
    parts = sorted(sorted(p["members"]) for p in spectral["partitions"])
    assert parts == [_LEFT, _RIGHT]
    # Partition ids are the rank of the smallest member, so a repeat run is identical.
    again = gc.graph.mine_topology("spectral", k=2, seed=7)
    assert again["partitions"] == spectral["partitions"]
    assert spectral["evidence"]["seed"] == 7

    analogy = gc.graph.mine_topology("analogy", k=4)
    assert analogy["matches"], "mirrored triangle nodes are an analogy"
    labels = {n: "Alpha" for n in _LEFT} | {n: "Beta" for n in _RIGHT}
    for match in analogy["matches"]:
        assert labels[match["a"]] != labels[match["b"]]
        assert 0.6 <= match["score"] <= 1.0

    synergy = gc.graph.mine_topology("synergy", k=3, seed=1)
    assert synergy["analytic"] == "synergy"
    assert all(len(b["communities"]) >= 2 for b in synergy["bridges"])

    candidates = [["tri-a1", "tri-a2"], ["tri-a1", "tri-b3"]]
    scored = gc.graph.mine_topology("hyperedge_score", candidates=candidates)
    assert [s["members"] for s in scored["scores"]] == candidates
    assert all(isinstance(s["score"], float) for s in scored["scores"])

    # The label and relationship narrow the graph the analytic reads: with only the
    # Alpha nodes left there are exactly 3 nodes and no bridge edge to cut through.
    alpha = gc.graph.mine_topology("mincut", label="Alpha")
    assert alpha["evidence"]["nodes"] == 3
    assert sorted(m for p in alpha["partitions"] for m in p["members"]) == _LEFT
    assert (
        gc.graph.mine_topology("mincut", relationship="LINKS")["evidence"]["edges"] == 7
    )


def test_kernel_refusals_are_typed(clean_graph):
    gc = clean_graph
    with pytest.raises(EngineResponseError, match="TOPOLOGY_TRIVIAL_GRAPH"):
        gc.graph.mine_topology("mincut")
    _seed(gc)
    for bad_k in (1, 257):
        with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
            gc.graph.mine_topology("spectral", k=bad_k)
    with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
        gc.graph.mine_topology("hyperedge_score", candidates=[["tri-a1", "ghost"]])
