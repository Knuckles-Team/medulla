"""Served code context and symptom diagnosis (MEDULLA-FEDERATED-QUERY-R654.3).

`CodeContext` and `Diagnose` read the caller's projected graph through the one
kernel in `medulla-plan`; this file stands up the served path and compares what
comes back with what the kernel computes for the same graph.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

_SYMBOLS = {
    "sym-run": "repo/a.rs::run",
    "sym-main": "repo/a.rs::main",
    "sym-parse": "repo/b.rs::parse",
    "sym-lex": "repo/b.rs::lex",
}
_CALLS = [
    ("sym-run", "sym-main"),
    ("sym-main", "sym-parse"),
    ("sym-main", "sym-lex"),
    ("sym-parse", "sym-lex"),
]


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _node(client: Any, graph: str, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _edge(client: Any, graph: str, src: str, tgt: str, **props: Any) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb(props),
        },
        graph,
    )


def _fixture(client: Any, graph: str) -> None:
    for node_id, name in _SYMBOLS.items():
        _node(
            client,
            graph,
            node_id,
            {"type": "Symbol", "symbol_type": "function", "qualified_name": name},
        )
    for caller, callee in _CALLS:
        _edge(client, graph, caller, callee, relationship="calls")
    _node(client, graph, "chg1", {"type": "Change", "at_ms": 10})
    _node(client, graph, "dep1", {"type": "Deployment", "at_ms": 20})
    _node(client, graph, "svc", {"type": "Service", "at_ms": 30})
    _node(client, graph, "anom", {"type": "HealthAnomaly", "at_ms": 40})
    _edge(
        client, graph, "anom", "svc", relationship="dependsOn", evidence_ids=["e-dep"]
    )
    _edge(client, graph, "chg1", "svc", relationship="changes", evidence_ids=["e-chg"])
    _edge(
        client, graph, "dep1", "svc", relationship="deployedTo", evidence_ids=["e-dpl"]
    )


def _unpack(result: Any) -> dict[str, Any]:
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _call(client: Any, graph: str, method: str, request: dict[str, Any]) -> Any:
    return _unpack(client._send(method, {"request": request}, graph))


def _with_graph(body: Any) -> None:
    graph = f"code-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _fixture(client, graph)
        body(client, graph)
    finally:
        client.tenants.delete(graph)
        client.close()


def _kernel_expected_code_context() -> dict[str, Any]:
    """The kernel's answer for `how` around `main`, derived from the fixture alone.

    Callers walk the call edges backward and callees forward, one hop count per
    symbol, ordered by hops then symbol; the hub of the visited subgraph is the
    only symbol on shortest paths between others.
    """
    main = _SYMBOLS["sym-main"]
    return {
        "anchors": [main],
        "usages": [{"symbol": _SYMBOLS["sym-run"], "hops": 1}],
        "impact": [
            {"symbol": _SYMBOLS["sym-lex"], "hops": 1},
            {"symbol": _SYMBOLS["sym-parse"], "hops": 1},
        ],
        "hub": main,
    }


# proves R654.3: Wiring
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R654.3")
def test_methods_reach_kernels(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R654.3: served output equals the kernels."""

    def body(client: Any, graph: str) -> None:
        expected = _kernel_expected_code_context()
        served = _call(
            client, graph, "CodeContext", {"anchor": "::main", "question": "how"}
        )
        assert served["status"] == "ok"
        assert served["anchors"] == expected["anchors"]
        assert served["usages"] == expected["usages"]
        assert served["impact"] == expected["impact"]
        assert served["candidates"] == []
        assert served["hubs"][0]["symbol"] == expected["hub"]
        assert served["hubs"][0]["score"] > 0.0

        usage = _call(
            client, graph, "CodeContext", {"anchor": "::lex", "question": "usage"}
        )
        assert [n["symbol"] for n in usage["usages"]] == [
            _SYMBOLS["sym-main"],
            _SYMBOLS["sym-parse"],
            _SYMBOLS["sym-run"],
        ]
        assert [n["hops"] for n in usage["usages"]] == [1, 1, 2]
        assert usage["impact"] == []

        diagnosis = _call(client, graph, "Diagnose", {"symptom": "anom"})
        assert diagnosis["symptom"] == "anom"
        assert [(link["stage"], link["node_id"]) for link in diagnosis["chain"]] == [
            ("change", "chg1"),
            ("deploy", "dep1"),
            ("dependency", "svc"),
            ("symptom", "anom"),
        ]
        by_node = {link["node_id"]: link for link in diagnosis["chain"]}
        assert by_node["chg1"]["relationship"] == "change"
        assert by_node["chg1"]["evidence_ids"] == ["e-chg"]
        assert by_node["svc"]["relationship"] == "depends_on"

    _with_graph(body)


# proves R654.4: Refusal and degraded answers
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R654.4")
def test_ambiguous_degraded_and_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R654.4: statuses, codes, no state change."""

    def body(client: Any, graph: str) -> None:
        before = _call(
            client, graph, "CodeContext", {"anchor": "::main", "question": "how"}
        )
        assert before["status"] == "ok"
        assert before["missing_repositories"] == []

        # Two symbols match the anchor: no guess, the candidates are listed.
        _node(
            client,
            graph,
            "sym-parse-2",
            {
                "type": "Symbol",
                "symbol_type": "function",
                "qualified_name": "repo/c.rs::parse",
            },
        )
        ambiguous = _call(
            client, graph, "CodeContext", {"anchor": "::parse", "question": "how"}
        )
        assert ambiguous["status"] == "ambiguous"
        assert ambiguous["anchors"] == []
        assert ambiguous["candidates"] == ["repo/b.rs::parse", "repo/c.rs::parse"]

        # A repository whose branches are no longer live degrades the answer.
        _node(
            client,
            graph,
            "branch-x",
            {"type": "Branch", "repository_id": "repo-x", "status": "stale"},
        )
        degraded = _call(
            client, graph, "CodeContext", {"anchor": "::main", "question": "how"}
        )
        assert degraded["status"] == "degraded"
        assert degraded["missing_repositories"] == ["repo-x"]
        assert degraded["anchors"] == before["anchors"]

        for request in (
            {"anchor": "::main", "question": "how", "depth": 7},
            {"anchor": "::nothing-matches", "question": "how"},
        ):
            with pytest.raises(Exception) as refused:
                _call(client, graph, "CodeContext", request)
            expected = (
                "CODE_DEPTH_EXCEEDED" if request.get("depth") else "CODE_ANCHOR_UNKNOWN"
            )
            assert expected in str(refused.value)
        with pytest.raises(Exception) as unknown:
            _call(client, graph, "Diagnose", {"symptom": "no-such-symptom"})
        assert "CODE_ANCHOR_UNKNOWN" in str(unknown.value)

        # The refusals changed nothing: the same read answers the same way.
        after = _call(
            client, graph, "CodeContext", {"anchor": "::main", "question": "how"}
        )
        assert after == degraded

    _with_graph(body)


# proves R654.5: Evidence
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R654.5")
def test_evidence_fields(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R654.5: hops, hub inputs, stage, evidence."""

    def body(client: Any, graph: str) -> None:
        served = _call(
            client, graph, "CodeContext", {"anchor": "::main", "question": "how"}
        )
        for node in served["usages"] + served["impact"]:
            assert set(node) == {"symbol", "hops"}
            assert node["hops"] >= 1
        hub = served["hubs"][0]
        assert set(hub) == {"symbol", "in_degree", "out_degree", "score"}
        assert hub["symbol"] == _SYMBOLS["sym-main"]
        assert (hub["in_degree"], hub["out_degree"]) == (1, 2)
        assert hub["score"] > 0.0

        diagnosis = _call(client, graph, "Diagnose", {"symptom": "anom"})
        assert diagnosis["chain"], "the chain names its links"
        for link in diagnosis["chain"]:
            assert set(link) == {"stage", "node_id", "relationship", "evidence_ids"}
        by_node = {link["node_id"]: link for link in diagnosis["chain"]}
        assert by_node["chg1"]["stage"] == "change"
        assert by_node["chg1"]["evidence_ids"] == ["e-chg"]
        assert by_node["dep1"]["stage"] == "deploy"
        assert by_node["dep1"]["evidence_ids"] == ["e-dpl"]
        assert by_node["svc"]["stage"] == "dependency"
        assert by_node["svc"]["evidence_ids"] == ["e-dep"]

    _with_graph(body)


# proves R654.6: Hot-path proof
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R654.6")
def test_code_context_and_diagnose(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R654.6: the hot path answers both methods."""

    def body(client: Any, graph: str) -> None:
        impact = _call(
            client,
            graph,
            "CodeContext",
            {"anchor": "::main", "question": "impact", "depth": 3},
        )
        assert impact["status"] == "ok"
        assert [n["symbol"] for n in impact["impact"]] == [
            _SYMBOLS["sym-lex"],
            _SYMBOLS["sym-parse"],
        ]
        assert impact["usages"] == []
        diagnosis = _call(client, graph, "Diagnose", {"symptom": "anom"})
        assert diagnosis["chain"][-1]["node_id"] == "anom"

    _with_graph(body)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R654.8")
def test_code_navigation_reads_the_index(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R654.8: nav and instances answer the index."""

    def body(client: Any, graph: str) -> None:
        _node(
            client,
            graph,
            "sym-cli",
            {
                "type": "Symbol",
                "symbol_type": "function",
                "qualified_name": "repo/c.rs::cli",
                "path": "c.rs",
                "line": 12,
            },
        )
        _edge(client, graph, "sym-cli", "sym-run", relationship="calls")

        nav = _call(
            client, graph, "CodeNavigation", {"action": "nav", "symbol": "::main"}
        )
        assert nav == {
            "items": [
                {
                    "symbol": _SYMBOLS["sym-main"],
                    "path": "",
                    "line": 0,
                    "kind": "function",
                }
            ]
        }
        callers = _call(
            client, graph, "CodeNavigation", {"action": "instances", "symbol": "::run"}
        )
        assert callers["items"] == [
            {"symbol": "repo/c.rs::cli", "path": "c.rs", "line": 12, "kind": "function"}
        ]
        listing = _call(client, graph, "CodeNavigation", {"action": "nav", "limit": 2})
        assert len(listing["items"]) == 2

        # An unknown symbol answers no items, as an absent one would; the request
        # shapes that cannot be answered refuse.
        unknown = _call(
            client, graph, "CodeNavigation", {"action": "nav", "symbol": "::absent"}
        )
        assert unknown == {"items": []}
        for bad in ({"action": "instances"}, {"action": "nav", "symbol": " "}):
            with pytest.raises(Exception, match="INVALID_ARGUMENT"):
                _call(client, graph, "CodeNavigation", bad)

        # The graph is the caller's: a second tenant's graph does not see these rows.
        other = f"code-{uuid.uuid4().hex[:8]}"
        client.tenants.create(other)
        try:
            empty = _call(
                client, other, "CodeNavigation", {"action": "nav", "symbol": "::main"}
            )
            assert empty == {"items": []}
        finally:
            client.tenants.delete(other)

    _with_graph(body)
