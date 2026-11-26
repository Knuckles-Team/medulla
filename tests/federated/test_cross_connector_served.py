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
