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
from conftest import request_context

ROOTS = ["Service", "SoftwareComponent", "Supplier", "Vulnerability"]

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


def _ask(client: Any, graph: str, roots: list[str]) -> dict:
    return _decode(client._send("SupplyChainImpact", {"roots": roots}, graph))


@pytest.mark.spec("MEDULLA-CQ-XSRC-R011")
def test_method_returns_typed_report(start_medulla_server) -> None:
    """Verifies MEDULLA-CQ-XSRC-R011: a typed report with rows, abstentions and refusals."""
    graph = f"supply-chain-impact-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _attach_seed(client, graph)
        _add_node(client, graph, "svc-checkout", {"type": "Service", "name": "checkout"})
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
