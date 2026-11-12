"""MEDULLA-FEDERATED-QUERY-R073.3 served acceptance: an `Mcp` source resolves.

Drives ``Method::RegisterForeignSource`` with an operation-bound ``Mcp`` spec and then a
served ``UnifiedQuery`` scan of it. The scan resolves to the connector-row seam (not the
unbound refusal of the old resolver). The engine holds no MCP client, so with no
connector port registered by the host the scan abstains with ``SOURCE_UNREACHABLE`` and
the refusal names the source label.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


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
