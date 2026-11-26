"""Served acceptance of raw-access sources (MEDULLA-FEDERATED-QUERY-R729).

A source registered with ``capabilities.raw`` is scanned by the connector, not by
the engine: ``raw_search`` finds candidates, ``raw_check_context`` keeps the ids the
caller may read, ``raw_read_span`` reads the kept spans. Each call carries the scan
bytes the query budget has left, and a connector that would scan past that answers
``OVER_CEILING``, which the engine refuses as the typed budget error. The gateway is
a loopback stub.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError

from .conftest import RAW_IDS, RAW_SERVER, WebGateway
from .test_web_metasearch_served import web_plan

RAW_SOURCE = {
    "Mcp": {"server": RAW_SERVER, "tool": "raw", "capabilities": {"raw": True}}
}
RAW_TOOLS = ["raw_search", "raw_check_context", "raw_read_span"]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R729")
def test_raw_source_search_is_budgeted_and_refuses_over_ceiling(
    clean_graph, web_gateway: WebGateway
):
    clean_graph.query.register_foreign_source("raw-docs", RAW_SOURCE)
    before = len(web_gateway.raw_calls)
    rows = clean_graph.query.unified(web_plan("raw-docs", "alpha"))
    # The caller is cleared for two of the three candidates; the third is never read.
    assert [row["id"] for row in rows if row["id"] in RAW_IDS] == [
        "raw-doc-1",
        "raw-doc-3",
    ]
    calls = web_gateway.raw_calls[before:]
    assert [call["tool"] for call in calls] == RAW_TOOLS
    assert calls[0]["arguments"]["query"] == "alpha"
    # Each call is told the bytes the budget has left: the earlier scans are spent.
    spent = web_gateway.raw_scan_bytes
    assert (
        calls[1]["max_scan_bytes"] == calls[0]["max_scan_bytes"] - spent["raw_search"]
    )
    assert calls[2]["max_scan_bytes"] == (
        calls[1]["max_scan_bytes"] - spent["raw_check_context"]
    )

    # A connector that would scan past the ceiling answers OVER_CEILING; the engine
    # refuses with the typed budget code and returns no rows.
    web_gateway.raw_scan_bytes["raw_search"] = 1 << 40
    try:
        with pytest.raises(EngineResponseError) as refused:
            clean_graph.query.unified(web_plan("raw-docs", "alpha"))
    finally:
        web_gateway.raw_scan_bytes["raw_search"] = 4096
    assert "FEDERATION_BUDGET_EXCEEDED:scan_bytes" in str(refused.value)
    # The refused scan stopped at the search; nothing was cleared or read after it.
    assert web_gateway.raw_calls[-1]["tool"] == "raw_search"
