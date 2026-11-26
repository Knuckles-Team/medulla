"""Topology analytics over the served wire (MEDULLA-FEDERATED-QUERY-R607.3).

The session fixture in conftest starts the native server. The fixture graph is two
triangles joined by one bridge edge, so every expected value is stated by hand from
the published definitions: the global minimum cut is the bridge (weight 1), the
spectral partition into two recovers the triangles, and the two mirrored triangle
nodes of different labels are an exact analogy.
"""

from __future__ import annotations

import re
import secrets
from typing import Any

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    server_timeout,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import EngineResponseError, SyncMedullaClient

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


# proves R607.3: Wiring
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


# ---------------------------------------------------------------------------
# Refusals and evidence (MEDULLA-FEDERATED-QUERY-R607.4, R607.5) run on their own
# engine: the size caps are set at their registry floors with `--knob`, and the
# engine's own log is read for the `medulla.topology.<analytic>` spans.
# ---------------------------------------------------------------------------

# The registry floors of `topology.max_nodes` and `topology.max_edges`.
_MAX_NODES = 1_000
_MAX_EDGES = 10_000
_BATCH = 200
_ANSI = re.compile(r"\x1b\[[0-9;]*m")


@pytest.fixture(scope="module")
def capped_engine(tmp_path_factory):
    """One engine with the topology size caps at their floors and a readable log."""
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    runtime = tmp_path_factory.mktemp("topology-capped")
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir()
    log_path = runtime / "engine.log"
    secret = secrets.token_hex(32)
    env = strict_server_env(
        str(runtime / "security"), auth_secret=secret, persist_dir=str(persist)
    )
    env.pop("RUST_LOG", None)
    process = spawn_server(
        [
            binary,
            "--socket-path",
            socket_path,
            "--knob",
            f"topology.max_nodes={_MAX_NODES}",
            "--knob",
            f"topology.max_edges={_MAX_EDGES}",
        ],
        env=env,
        log_path=log_path,
    )
    try:
        wait_for_server(
            process, socket_path, name="topology capped engine", log_path=log_path
        )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=secret,
            verified_context=bootstrap_context(),
        ) as bootstrap:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=secret,
            verified_context=request_context(),
            timeout=server_timeout(),
        ) as client:
            yield client, log_path
    finally:
        stop_server(process, name="topology capped engine", log_path=log_path)


def _span_lines(log_path, analytic: str) -> list[str]:
    text = _ANSI.sub("", log_path.read_text("utf-8", "replace"))
    return [
        line
        for line in text.splitlines()
        if f"medulla.topology.{analytic}{{" in line
        and "topology analytic finished" in line
    ]


# proves R607.4: Refusal
# spec: MEDULLA-FEDERATED-QUERY-R607.4
def test_topology_refusals(capped_engine):
    client, _ = capped_engine
    client.graph.clear()

    # TOPOLOGY_TRIVIAL_GRAPH: fewer than 3 nodes.
    client.nodes.add("lone-a", {"type": "Alpha"})
    client.nodes.add("lone-b", {"type": "Alpha"})
    with pytest.raises(EngineResponseError, match="TOPOLOGY_TRIVIAL_GRAPH"):
        client.graph.mine_topology("mincut")

    # TOPOLOGY_PARAM_INVALID: k outside [2, 256], and an edge weight that is not a
    # finite positive number.
    client.graph.clear()
    _seed(client)
    for bad_k in (1, 257):
        with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
            client.graph.mine_topology("spectral", k=bad_k)
    client.edges.add("tri-a1", "tri-b3", {"relationship": "BAD", "weight": -1.0})
    with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
        client.graph.mine_topology("mincut")
    with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
        client.graph.mine_topology("spectral", k=2)
    # The relationship filter leaves the bad edge out, so the same graph reads.
    assert (
        client.graph.mine_topology("mincut", relationship="LINKS")["evidence"][
            "cut_weight"
        ]
        == 1.0
    )

    # TOPOLOGY_GRAPH_TOO_LARGE: one node over `topology.max_nodes`.
    client.graph.clear()
    ids = [f"big-{i:05d}" for i in range(_MAX_NODES + 1)]
    for start in range(0, len(ids), _BATCH):
        client.lifecycle.batch_update(
            [
                {"op": "add_node", "id": node, "properties": {"type": "Big"}}
                for node in ids[start : start + _BATCH]
            ]
        )
    with pytest.raises(EngineResponseError, match="TOPOLOGY_GRAPH_TOO_LARGE"):
        client.graph.mine_topology("mincut")
    # A refusal writes nothing: a label-narrowed read sees exactly the nodes
    # written after it.
    client.nodes.add("small-a", {"type": "Small"})
    client.nodes.add("small-b", {"type": "Small"})
    client.nodes.add("small-c", {"type": "Small"})
    client.edges.add("small-a", "small-b", {"relationship": "LINKS"})
    client.edges.add("small-b", "small-c", {"relationship": "LINKS"})
    small = client.graph.mine_topology("mincut", label="Small")
    assert small["evidence"]["nodes"] == 3


# proves R607.5: Evidence
# spec: MEDULLA-FEDERATED-QUERY-R607.5
def test_evidence_fields_after_success_and_refusal(capped_engine):
    client, log_path = capped_engine
    client.graph.clear()
    _seed(client)

    spectral = client.graph.mine_topology("spectral", k=2, seed=11)
    evidence = spectral["evidence"]
    assert set(evidence) == {
        "nodes",
        "edges",
        "iterations",
        "modularity",
        "cut_weight",
        "match_scores",
        "elapsed_ms",
        "seed",
    }
    assert evidence["nodes"] == 6
    assert evidence["edges"] == 7
    assert evidence["seed"] == 11
    assert evidence["iterations"] > 0
    assert isinstance(evidence["modularity"], float)
    assert evidence["cut_weight"] is None
    assert evidence["elapsed_ms"] >= 0

    mincut = client.graph.mine_topology("mincut")["evidence"]
    assert mincut["cut_weight"] == 1.0
    assert mincut["modularity"] is None

    analogy = client.graph.mine_topology("analogy", k=4)
    assert analogy["evidence"]["match_scores"] == [
        m["score"] for m in analogy["matches"]
    ]

    with pytest.raises(EngineResponseError, match="TOPOLOGY_PARAM_INVALID"):
        client.graph.mine_topology("spectral", k=1)

    # The engine's own spans carry the same counts, and the refusal its outcome.
    spectral_spans = _span_lines(log_path, "spectral")
    ok = [line for line in spectral_spans if "outcome=" in line and "ok" in line]
    assert any(
        "nodes=6" in line and "edges=7" in line and "seed=11" in line for line in ok
    ), spectral_spans
    assert any(
        "refused" in line and "TOPOLOGY_PARAM_INVALID" in line
        for line in spectral_spans
    ), spectral_spans
    assert any("nodes=6" in line for line in _span_lines(log_path, "mincut"))


# proves R607.6: Hot-path proof
# spec: MEDULLA-FEDERATED-QUERY-R607.6
# proves R607: Topology analytics find partitions, analogies, synergy and hyperedge
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R607")
def test_each_analytic_is_served(clean_graph):
    """All five analytics answer over the wire on ONE fixture, each naming itself."""
    gc = clean_graph
    _seed(gc)
    candidates = [["tri-a1", "tri-a2"], ["tri-a1", "tri-b3"]]
    requests: dict[str, dict[str, Any]] = {
        "spectral": {"k": 2, "seed": 7},
        "mincut": {},
        "analogy": {"k": 4},
        "synergy": {"k": 3, "seed": 1},
        "hyperedge_score": {"candidates": candidates},
    }
    answers = {
        analytic: gc.graph.mine_topology(analytic, **params)
        for analytic, params in requests.items()
    }
    for analytic, answer in answers.items():
        assert answer["analytic"] == analytic
        assert answer["evidence"], analytic
    assert sorted(sorted(p["members"]) for p in answers["spectral"]["partitions"]) == [
        _LEFT,
        _RIGHT,
    ]
    assert answers["mincut"]["evidence"]["cut_weight"] == 1.0
    assert answers["analogy"]["matches"]
    assert all(len(b["communities"]) >= 2 for b in answers["synergy"]["bridges"])
    assert [s["members"] for s in answers["hyperedge_score"]["scores"]] == candidates
