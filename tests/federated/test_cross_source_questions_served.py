"""Served cross-source competency questions (MEDULLA-CQ-XSRC-R008.4).

`CrossSourceQuestion` plans one of the three questions over the graph's
`vg:SourceAuthority` rows and returns the typed report. An unknown question name
refuses with `CQ_XSRC_UNKNOWN_QUESTION`; a known question on a graph that declares
no authority rows still answers, with its classes as typed abstentions.
"""

from __future__ import annotations

import os
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import request_context

QUESTIONS = ("exposed_services", "host_limit_capabilities", "contract_incidents")


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
    # NodeClient.add writes to the connection's graph; this names the test graph.
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _ask(client: Any, graph: str, question: str) -> dict:
    return _decode(client._send("CrossSourceQuestion", {"question": question}, graph))


@pytest.mark.spec("MEDULLA-CQ-XSRC-R008.4")
def test_unknown_question_refuses(start_medulla_server) -> None:
    """Verifies MEDULLA-CQ-XSRC-R008.4: both refusal codes and a served report."""
    graph = "cross-source-questions-refusal"
    client = _client()
    client.tenants.create(graph)
    try:
        with pytest.raises(Exception, match="CQ_XSRC_UNKNOWN_QUESTION"):
            _ask(client, graph, "everything")
        # A graph with no subject nodes answers every question with no items.
        for question in QUESTIONS:
            report = _ask(client, graph, question)
            assert report["question"] == question
            assert report["items"] == []
        # A subject node that names its source is one item carrying that source.
        _add_node(
            client,
            graph,
            "svc-checkout",
            {"type": "Service", "name": "checkout", "source": "uptime-mcp"},
        )
        report = _ask(client, graph, "exposed_services")
        assert [(item["subject_iri"], item["source"]) for item in report["items"]] == [
            ("svc-checkout", "uptime-mcp")
        ]
        assert {"relation": "name", "value": "checkout"} in report["items"][0][
            "related"
        ]
        # A subject node with no source refuses the answer; it is never defaulted.
        _add_node(client, graph, "svc-orphan", {"type": "Service", "name": "orphan"})
        with pytest.raises(Exception, match="CQ_XSRC_ITEM_WITHOUT_SOURCE"):
            _ask(client, graph, "exposed_services")
    finally:
        client.tenants.delete(graph)
        client.close()


SEED = (
    Path(__file__).resolve().parents[2]
    / "crates/medulla-core/tests/fixtures/cross_source_authority_seed.ttl"
).read_text(encoding="utf-8")

# (question, class that the unconnected source would serve, source label)
UNCONNECTED_CASES = {
    "vulnerable_image_services_unconnected": (
        "exposed_services",
        "Vulnerability",
        "gitlab-mcp",
    ),
    "host_limit_capabilities_unconnected": (
        "host_limit_capabilities",
        "Host",
        "lgtm-mcp",
    ),
    "contract_incidents_unconnected": (
        "contract_incidents",
        "CustomerContract",
        "salesforce-mcp",
    ),
}


@pytest.mark.spec("MEDULLA-CQ-XSRC-R008.1.2")
@pytest.mark.spec("MEDULLA-CQ-XSRC-R008.2.4")
@pytest.mark.spec("MEDULLA-CQ-XSRC-R008.3.4")
@pytest.mark.parametrize("case", sorted(UNCONNECTED_CASES))
def test_question_served(start_medulla_server, case: str) -> None:
    """The seeded authority rows name sources no approved mapping connects, so the
    question answers normally with the unconnected source as a typed abstention
    and no item (MEDULLA-CQ-XSRC-R006.3)."""
    question, class_name, source = UNCONNECTED_CASES[case]
    graph = f"cross-source-{case.replace('_', '-')}-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
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
        report = _ask(client, graph, question)
        assert report["question"] == question
        assert report["items"] == []
        unconnected = [
            abstention
            for abstention in report["abstentions"]
            if abstention["kind"] == "unconnected"
            and abstention["class"].endswith(class_name)
        ]
        assert unconnected, report["abstentions"]
        assert any(entry["source"] == source for entry in unconnected), unconnected
        assert all("reading" not in repr(item) for item in report["items"])
    finally:
        client.tenants.delete(graph)
        client.close()
