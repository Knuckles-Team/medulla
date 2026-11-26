"""Cross-connector answers of the supply-chain impact method (MEDULLA-CQ-XSRC-R012).

The served `SupplyChainImpact` method answers over the facts of several connectors. A
source the report cannot use (its role is inspiration, which cannot answer a question
about the present) contributes no fact and is named in the abstentions with the reason
`role_insufficient`; the answer is returned, never refused.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import register_commons_agents, request_context

CORE = "https://knuckles-team.github.io/ontology/core#"
SOURCE = {"Mcp": {"server": "cross-connector-test-mcp", "tool": "search"}}

# Two connectors: the code host serves software components but its role is
# inspiration; the procurement system serves suppliers as live truth.
ROLE_SEED = """\
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsrole: <https://knuckles-team.github.io/ontology/source-record/role-seed#> .

xsrole:erpnext a vg:SourceConnection ; rdfs:label "erpnext-mcp" ;
    vg:sourceRole "live_truth" .
xsrole:github a vg:SourceConnection ; rdfs:label "github-mcp" ;
    vg:sourceRole "inspiration" .

xsrole:supplier-erpnext a vg:SourceAuthority ;
    vg:authoritativeSource xsrole:erpnext ;
    vg:authorityFor :Supplier ;
    vg:authorityRank 1 .

xsrole:software-component-github a vg:SourceAuthority ;
    vg:authoritativeSource xsrole:github ;
    vg:authorityFor :SoftwareComponent ;
    vg:authorityRank 1 .
"""


# The join scenario: the procurement system serves suppliers, the architecture
# system serves software components, both as live truth.
JOIN_SEED = """\
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsjoin: <https://knuckles-team.github.io/ontology/source-record/join-seed#> .

xsjoin:erpnext a vg:SourceConnection ; rdfs:label "erpnext-mcp" ;
    vg:sourceRole "live_truth" .
xsjoin:leanix a vg:SourceConnection ; rdfs:label "leanix-mcp" ;
    vg:sourceRole "live_truth" .

xsjoin:supplier-erpnext a vg:SourceAuthority ;
    vg:authoritativeSource xsjoin:erpnext ;
    vg:authorityFor :Supplier ;
    vg:authorityRank 1 .

xsjoin:software-component-leanix a vg:SourceAuthority ;
    vg:authoritativeSource xsjoin:leanix ;
    vg:authorityFor :SoftwareComponent ;
    vg:authorityRank 1 .
"""

READ_AT_MS = 1_764_000_000_123


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


@pytest.mark.spec("MEDULLA-CQ-XSRC-R012.2")
def test_unreadable_source_is_excluded_and_named_in_coverage(
    start_medulla_server,
) -> None:
    """Verifies MEDULLA-CQ-XSRC-R012.2: no fact of the excluded source, one coverage
    entry naming it `role_insufficient`, and the facts of the readable source stay."""
    graph = f"cross-connector-role-{uuid.uuid4().hex[:8]}"
    system = _client()
    system.tenants.create(graph)
    agent = f"xsrc-role-{uuid.uuid4().hex[:8]}"
    caller = None
    try:
        system._send(
            "GraphSchema",
            {
                "op": {
                    "op": "attach",
                    "source_id": f"admin:role-seed-{uuid.uuid4().hex[:8]}",
                    "ontology_ttl": ROLE_SEED,
                }
            },
            graph,
            idempotency_key=f"attach-{uuid.uuid4().hex}",
        )
        _add_node(system, graph, "svc-checkout", {"type": "Service"})
        _add_node(
            system,
            graph,
            "ver-from-github",
            {"type": "ComponentVersion", "source": "github-mcp"},
        )
        _add_node(
            system,
            graph,
            "ver-from-erpnext",
            {"type": "ComponentVersion", "source": "erpnext-mcp"},
        )
        _add_node(system, graph, "cmp-nginx", {"type": "SoftwareComponent"})
        _add_node(system, graph, "sup-acme", {"type": "Supplier"})
        for version in ("ver-from-github", "ver-from-erpnext"):
            _add_edge(system, graph, "svc-checkout", "usesComponent", version)
            _add_edge(system, graph, version, "supplyChainVersionOf", "cmp-nginx")
        _add_edge(system, graph, "cmp-nginx", "publishedBy", "sup-acme")

        register_commons_agents(agent, graph=graph)
        from medulla.client import SyncMedullaClient

        caller = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=agent),
        )
        for name in ("erpnext-mcp", "github-mcp"):
            caller._send(
                "RegisterForeignSource",
                {"name": name, "source": SOURCE},
                None,
                idempotency_key=f"register-{name}-{uuid.uuid4().hex}",
            )

        report = _decode(
            caller._send(
                "SupplyChainImpact",
                {"roots": ["Supplier", "SoftwareComponent"]},
                graph,
            )
        )
        # The excluded source's facts are gone; the readable source's facts stay.
        sources = [
            item["source"]
            for impact in report["service_impacts"]
            for item in impact["items"]
        ]
        assert sources == ["erpnext-mcp"], report
        # It is named once, with its reason and the class it could not serve.
        named = [
            entry
            for entry in report["abstentions"]
            if entry.get("kind") == "role_insufficient"
        ]
        assert named == [
            {
                "kind": "role_insufficient",
                "source": "github-mcp",
                "class": f"{CORE}SoftwareComponent",
            }
        ], report["abstentions"]
        assert all(
            entry["source"] != "github-mcp" for entry in report["sources_used"]
        ), report["sources_used"]
    finally:
        if caller is not None:
            caller.close()
        system.tenants.delete(graph)
        system.close()


def _supplier_ids(report: dict[str, Any]) -> list[str]:
    return sorted(
        {
            item["supplier"]
            for impact in report["service_impacts"]
            for item in impact["items"]
        }
    )


@pytest.mark.spec("MEDULLA-CQ-XSRC-R012.1")
def test_join_across_two_virtual_graphs_carries_provenance(
    start_medulla_server,
) -> None:
    """Verifies MEDULLA-CQ-XSRC-R012.1: the supplier of the first connector joins the
    component version of the second only across an approved `owl:sameAs` link, and every
    fact of the non-empty answer names its source, its mapping and its read time."""
    tag = uuid.uuid4().hex[:8]
    graph = f"cross-connector-join-{tag}"
    system = _client()
    system.tenants.create(graph)
    agent = f"xsrc-join-{tag}"
    caller = None
    erpnext_supplier = f"erpnext:SUP-{tag}"
    leanix_supplier = f"leanix:fs-{tag}"
    try:
        system._send(
            "GraphSchema",
            {
                "op": {
                    "op": "attach",
                    "source_id": f"admin:join-seed-{tag}",
                    "ontology_ttl": JOIN_SEED,
                }
            },
            graph,
            idempotency_key=f"attach-{uuid.uuid4().hex}",
        )
        _add_node(system, graph, "svc-billing", {"type": "Service"})
        _add_node(
            system,
            graph,
            "ver-leanix",
            {
                "type": "ComponentVersion",
                "source": "leanix-mcp",
                "mappingRef": "vm:leanix-components",
                "readAtMs": READ_AT_MS,
            },
        )
        _add_node(system, graph, "cmp-nginx", {"type": "SoftwareComponent"})
        for supplier, name in (
            (erpnext_supplier, "Acme Corp"),
            (leanix_supplier, "Acme Corp"),
        ):
            _add_node(
                system,
                graph,
                supplier,
                {"type": "Supplier", "name": name, "host": "acme.example"},
            )
        _add_edge(system, graph, "svc-billing", "usesComponent", "ver-leanix")
        _add_edge(system, graph, "ver-leanix", "supplyChainVersionOf", "cmp-nginx")
        _add_edge(system, graph, "cmp-nginx", "publishedBy", leanix_supplier)

        register_commons_agents(agent, graph=graph)
        from medulla.client import SyncMedullaClient

        caller = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=agent),
        )
        for name in ("erpnext-mcp", "leanix-mcp"):
            caller._send(
                "RegisterForeignSource",
                {"name": name, "source": SOURCE},
                None,
                idempotency_key=f"register-{name}-{uuid.uuid4().hex}",
            )

        params = {"roots": ["Supplier", "SoftwareComponent"]}
        # Before the approval the first connector's supplier joins no row.
        before = _decode(caller._send("SupplyChainImpact", params, graph))
        assert _supplier_ids(before) == [leanix_supplier], before

        system._send(
            "ApproveEntityLink",
            {
                "left": {"application": "erpnext", "entity_id": f"SUP-{tag}"},
                "right": {"application": "leanix", "entity_id": f"fs-{tag}"},
            },
            graph,
        )
        after = _decode(caller._send("SupplyChainImpact", params, graph))
        assert _supplier_ids(after) == [erpnext_supplier, leanix_supplier], after
        items = [
            item for impact in after["service_impacts"] for item in impact["items"]
        ]
        assert items, after
        for item in items:
            assert item["source"] == "leanix-mcp", item
            assert item["mapping"] == "vm:leanix-components", item
            assert item["read_at_ms"] == READ_AT_MS, item
    finally:
        if caller is not None:
            caller.close()
        system.tenants.delete(graph)
        system.close()


@pytest.mark.spec("MEDULLA-CQ-XSRC-R012")
def test_join_and_coverage_for_the_final_phase_scenario(start_medulla_server) -> None:
    """Verifies MEDULLA-CQ-XSRC-R012: the final phase scenario's engine side, the joined
    answer with its provenance (`.1`) and the coverage block naming an unreadable source
    (`.2`), each over its own graph through the served `SupplyChainImpact`."""
    test_join_across_two_virtual_graphs_carries_provenance(start_medulla_server)
    test_unreadable_source_is_excluded_and_named_in_coverage(start_medulla_server)
