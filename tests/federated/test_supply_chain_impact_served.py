"""Served supply-chain impact report (MEDULLA-CQ-XSRC-R011).

`SupplyChainImpact` plans the named class roots over the graph's `vg:SourceAuthority`
rows and the caller's own foreign catalog, derives the impact rows from the nodes and
edges the caller may read, and returns the typed report. An authority source the caller
has not registered is a typed abstention in the answer, never an error.
"""

from __future__ import annotations

import os
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import register_commons_agents, request_context

ROOTS = ["Service", "SoftwareComponent", "Supplier", "Vulnerability"]

CORE = "https://knuckles-team.github.io/ontology/core#"
SOURCE = {"Mcp": {"server": "coverage-test-mcp", "tool": "search"}}
PROCUREMENT = {f"{CORE}Supplier", f"{CORE}SupplyChainItem"}

SEED = (
    Path(__file__).resolve().parents[2]
    / "crates/medulla-core/tests/fixtures/cross_source_authority_seed.ttl"
).read_text(encoding="utf-8")


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _add_node(client: Any, graph: str, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _add_edge(client: Any, graph: str, source: str, relation: str, target: str) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": msgpack.packb({"relationship": relation}),
        },
        graph,
    )


def _attach_seed(client: Any, graph: str) -> None:
    client._send(
        "GraphSchema",
        {
            "op": {
                "op": "attach",
                "source_id": f"admin:xsrc-seed-{uuid.uuid4().hex[:8]}",
                "ontology_ttl": SEED,
            }
        },
        graph,
        idempotency_key=f"attach-{uuid.uuid4().hex}",
    )


def _ask(
    client: Any, graph: str, roots: list[str], contracts: list[str] | None = None
) -> dict:
    params: dict[str, Any] = {"roots": roots}
    if contracts is not None:
        params["contracts"] = contracts
    return _decode(client._send("SupplyChainImpact", params, graph))


@pytest.mark.spec("MEDULLA-CQ-XSRC-R011")
def test_method_returns_typed_report(start_medulla_server) -> None:
    """Verifies MEDULLA-CQ-XSRC-R011: a typed report, abstentions and refusals."""
    graph = f"supply-chain-impact-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _attach_seed(client, graph)
        _add_node(
            client, graph, "svc-checkout", {"type": "Service", "name": "checkout"}
        )
        _add_node(
            client,
            graph,
            "ver-nginx-1.18",
            {"type": "ComponentVersion", "source": "github-mcp"},
        )
        _add_node(client, graph, "cmp-nginx", {"type": "SoftwareComponent"})
        _add_node(client, graph, "sup-acme", {"type": "Supplier"})
        _add_edge(client, graph, "svc-checkout", "usesComponent", "ver-nginx-1.18")
        _add_edge(client, graph, "ver-nginx-1.18", "supplyChainVersionOf", "cmp-nginx")
        _add_edge(client, graph, "cmp-nginx", "publishedBy", "sup-acme")

        report = _ask(client, graph, ROOTS)
        assert set(report) == {"service_impacts", "sources_used", "abstentions"}
        assert [impact["service"] for impact in report["service_impacts"]] == [
            "svc-checkout"
        ]
        item = report["service_impacts"][0]["items"][0]
        assert item["supplier"] == "sup-acme"
        assert item["component_version"] == "ver-nginx-1.18"
        assert item["source"] == "github-mcp"
        # No authority source is registered in the caller's catalog, so every class
        # abstains by name; the answer is returned, never an error.
        assert report["abstentions"]
        assert all(
            {"kind", "source", "class"} <= set(entry) for entry in report["abstentions"]
        )
        assert report["sources_used"] == []

        with pytest.raises(Exception, match="CQ_CLASS_UNKNOWN"):
            _ask(client, graph, ["Gadget"])
        with pytest.raises(Exception, match="PARAMETER_INVALID"):
            _ask(client, graph, [])

        # A component version with no source refuses the answer; it is never defaulted.
        _add_node(client, graph, "ver-orphan", {"type": "ComponentVersion"})
        _add_edge(client, graph, "svc-checkout", "usesComponent", "ver-orphan")
        with pytest.raises(Exception, match="CQ_XSRC_ITEM_WITHOUT_SOURCE"):
            _ask(client, graph, ROOTS)
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-SUPPLY-VOCAB-R005.9")
def test_uncovered_contract_abstains_until_link_is_approved(
    start_medulla_server,
) -> None:
    """Verifies MEDULLA-SUPPLY-VOCAB-R005.9: a contract with no `coversService` edge is
    `contract_uncovered`; once a steward's approved link is written the report lists the
    service; a contract that names no node is refused."""
    graph = f"supply-chain-contract-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _attach_seed(client, graph)
        _add_node(
            client, graph, "svc-checkout", {"type": "Service", "name": "checkout"}
        )
        _add_node(client, graph, "contract-1", {"type": "CustomerContract"})

        before = _ask(client, graph, ROOTS, ["contract-1"])
        assert "contract_coverage" not in before
        assert {"kind": "contract_uncovered", "contract": "contract-1"} in before[
            "abstentions"
        ]

        # The steward's approved change request writes the edge through AddEdge.
        _add_edge(client, graph, "contract-1", "coversService", "svc-checkout")
        after = _ask(client, graph, ROOTS, ["contract-1"])
        assert after["contract_coverage"] == [
            {"contract": "contract-1", "services": ["svc-checkout"]}
        ]
        assert {"kind": "contract_uncovered", "contract": "contract-1"} not in after[
            "abstentions"
        ]

        # No contract asked is the base report.
        assert "contract_coverage" not in _ask(client, graph, ROOTS)
        with pytest.raises(Exception, match="PARAMETER_INVALID"):
            _ask(client, graph, ROOTS, ["contract-missing"])
    finally:
        client.tenants.delete(graph)
        client.close()


def _agent_client(agent: str, graph: str) -> Any:
    """A client for a fresh principal that reads `graph`, with its own catalog."""
    from medulla.client import SyncMedullaClient

    register_commons_agents(agent, graph=graph)
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent),
    )


def _register(client: Any, *names: str) -> None:
    for name in names:
        client._send(
            "RegisterForeignSource",
            {"name": name, "source": SOURCE},
            None,
            idempotency_key=f"register-{name}-{uuid.uuid4().hex}",
        )


@pytest.mark.spec("MEDULLA-CQ-XSRC-R007.3")
def test_erpnext_absent_abstains_and_software_rows_unchanged(
    start_medulla_server,
) -> None:
    """Verifies MEDULLA-CQ-XSRC-R007.3: with the procurement source unconnected the
    report names the abstention and uses no Supplier source; connecting it removes
    only that."""
    graph = f"supply-chain-erpnext-{uuid.uuid4().hex[:8]}"
    system = _client()
    system.tenants.create(graph)
    agent = f"xsrc-erpnext-{uuid.uuid4().hex[:8]}"
    caller = None
    try:
        _attach_seed(system, graph)
        _add_node(
            system, graph, "svc-checkout", {"type": "Service", "name": "checkout"}
        )
        _add_node(
            system,
            graph,
            "ver-nginx-1.18",
            {"type": "ComponentVersion", "source": "github-mcp"},
        )
        _add_node(system, graph, "cmp-nginx", {"type": "SoftwareComponent"})
        _add_node(system, graph, "sup-acme", {"type": "Supplier"})
        _add_edge(system, graph, "svc-checkout", "usesComponent", "ver-nginx-1.18")
        _add_edge(system, graph, "ver-nginx-1.18", "supplyChainVersionOf", "cmp-nginx")
        _add_edge(system, graph, "cmp-nginx", "publishedBy", "sup-acme")

        caller = _agent_client(agent, graph)
        _register(
            caller,
            "leanix-mcp",
            "github-mcp",
            "gitlab-mcp",
            "dockerhub-mcp",
            "uptime-mcp",
        )

        without = _ask(caller, graph, ROOTS)
        missing = {
            "kind": "unconnected",
            "source": "erpnext-mcp",
            "class": f"{CORE}Supplier",
        }
        assert missing in without["abstentions"], without["abstentions"]
        assert all(
            entry["class_served"] != f"{CORE}Supplier"
            for entry in without["sources_used"]
        )

        _register(caller, "erpnext-mcp")
        with_erpnext = _ask(caller, graph, ROOTS)
        assert missing not in with_erpnext["abstentions"], with_erpnext["abstentions"]

        def software_sources(report: dict) -> list[dict]:
            return [
                e
                for e in report["sources_used"]
                if e["class_served"] not in PROCUREMENT
            ]

        def other_items(report: dict) -> list[dict]:
            return [
                item
                for impact in report["service_impacts"]
                for item in impact["items"]
                if item["source"] != "erpnext-mcp"
            ]

        assert software_sources(without) == software_sources(with_erpnext)
        assert other_items(without) == other_items(with_erpnext)
    finally:
        if caller is not None:
            caller.close()
        system.tenants.delete(graph)
        system.close()
