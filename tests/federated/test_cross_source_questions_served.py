"""Served cross-source competency questions (MEDULLA-CQ-XSRC-R008.4).

`CrossSourceQuestion` plans one of the three questions over the graph's
`vg:SourceAuthority` rows and returns the typed report. An unknown question name
refuses with `CQ_XSRC_UNKNOWN_QUESTION`; a known question on a graph that declares
no authority rows still answers, with its classes as typed abstentions.
"""

from __future__ import annotations

import os
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
        client.nodes.add(
            "svc-checkout",
            {"type": "Service", "name": "checkout", "source": "uptime-mcp"},
            graph=graph,
        )
        report = _ask(client, graph, "exposed_services")
        assert [(item["subject_iri"], item["source"]) for item in report["items"]] == [
            ("svc-checkout", "uptime-mcp")
        ]
        assert {"relation": "name", "value": "checkout"} in report["items"][0][
            "related"
        ]
        # A subject node with no source refuses the answer; it is never defaulted.
        client.nodes.add(
            "svc-orphan", {"type": "Service", "name": "orphan"}, graph=graph
        )
        with pytest.raises(Exception, match="CQ_XSRC_ITEM_WITHOUT_SOURCE"):
            _ask(client, graph, "exposed_services")
    finally:
        client.tenants.delete(graph)
        client.close()
