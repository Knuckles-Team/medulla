"""Served acceptance of web metasearch sources.

MEDULLA-FEDERATED-QUERY-R073.3: an `Mcp` source resolves.

Drives ``Method::RegisterForeignSource`` with an operation-bound ``Mcp`` spec and then a
served ``UnifiedQuery`` scan of it. The scan resolves to the connector-row seam (not the
unbound refusal of the old resolver). The engine holds no MCP client, so with no
connector port registered by the host the scan abstains with ``SOURCE_UNREACHABLE`` and
the refusal names the source label.

MEDULLA-UNIFIED-DATA-PLANE-R038.7: a pattern over a registered web source becomes
one connector call through the fleet gateway, repeats inside the TTL are served from
the owner-scoped cache, and the graph store holds no result row. The gateway is a
loopback stub that replays ``web_search_recorded.json`` and counts its calls.
"""

from __future__ import annotations

import os
import time

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import request_context
except ImportError:  # pragma: no cover - package-style collection
    from tests.conftest import request_context

from .conftest import RECORDED, WEB_SERVER, WEB_TOOL, WebGateway

WEB_SOURCE = {"Mcp": {"server": WEB_SERVER, "tool": WEB_TOOL}}
URLS = [row["id"] for row in RECORDED["rows"]]


def scan(name: str) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": False}}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R073.3")
def test_mcp_source_resolves(clean_graph):
    registered = clean_graph.query.register_foreign_source(
        "web-metasearch-source",
        {"Mcp": {"server": "searxng-mcp", "tool": "web_search"}},
    )
    assert registered == "web-metasearch-source"

    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.unified([scan("web-metasearch-source")])
    message = str(refused.value)
    assert "SOURCE_UNREACHABLE" in message
    assert "mcp#" in message
    assert "Oq2Unbound" not in message


def web_plan(name: str, text: str = "needle") -> list[dict[str, object]]:
    return [
        {
            "FuseRrf": {
                "branches": [
                    [{"RankText": {"query": text}}],
                    [
                        {
                            "ForeignScan": {
                                "source": {"Named": {"name": name}},
                                "join": False,
                            }
                        }
                    ],
                ],
                "k": 0.0,
            }
        }
    ]


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.7.6")
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.7.1")
def test_pattern_pushes_to_connector_with_ttl_cache(
    clean_graph, web_gateway: WebGateway
):
    clean_graph.query.register_foreign_source("web-ttl", WEB_SOURCE)
    before = web_gateway.row_calls
    first = clean_graph.query.unified(web_plan("web-ttl"))
    # Typed rows: the id is the result URL, in the recorded rank order.
    assert [row["id"] for row in first if row["id"] in URLS] == URLS
    again = clean_graph.query.unified(web_plan("web-ttl"))
    assert [row["id"] for row in again if row["id"] in URLS] == URLS
    assert web_gateway.row_calls - before == 1, "the repeat inside the TTL is cached"
    sent = web_gateway.calls[-1]["arguments"]
    assert sent["query"] == "needle"
    time.sleep(2.5)
    clean_graph.query.unified(web_plan("web-ttl"))
    assert web_gateway.row_calls - before == 2, "one more call after the TTL"
    # The engine copies no result into the graph store.
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == []
    assert clean_graph.query.unified([{"Scan": {"label": "Doc"}}]) == []


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.7.3")
def test_cache_is_owner_scoped(clean_graph, web_gateway: WebGateway):
    clean_graph.query.register_foreign_source("web-owner", WEB_SOURCE)
    clean_graph.query.unified(web_plan("web-owner", "scoped"))
    mine = web_gateway.row_calls
    clean_graph.query.unified(web_plan("web-owner", "scoped"))
    assert web_gateway.row_calls == mine, "the same owner reads from the cache"
    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="web-owner-two"),
    )
    other.query.register_foreign_source("web-owner", WEB_SOURCE)
    other.query.unified(web_plan("web-owner", "scoped"))
    assert web_gateway.row_calls == mine + 1, "another owner never shares an entry"


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.7.7")
def test_candidates_not_stored(clean_graph, web_gateway: WebGateway):
    clean_graph.query.register_foreign_source("web-cand", WEB_SOURCE)
    rows = clean_graph.query.unified(web_plan("web-cand", "candidates"))
    assert set(URLS) <= {row["id"] for row in rows}
    # Results are fetch candidates: no node or triple holds a result URL afterwards.
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == []
