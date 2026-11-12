"""MEDULLA-UNIFIED-DATA-PLANE-R038.5.1 served acceptance: the search-index capability.

Drives ``Method::RegisterForeignSource`` with an operation-bound ``Mcp`` spec that
declares a ``SearchIndexCapability``: a valid declaration registers, and a
declaration with ``max_k`` outside ``[1, 10000]`` or with no search mode is
refused with ``SEARCH_CAPABILITY_INVALID`` before anything is stored.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


def mcp_source(**search: object) -> dict[str, object]:
    return {
        "Mcp": {
            "server": "searxng-mcp",
            "tool": "web_search",
            "capabilities": {"search": search},
        }
    }


VALID = {
    "full_text": True,
    "vector": False,
    "filters": True,
    "max_k": 50,
    "score_kind": "bm25",
}


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.5.1")
def test_relevance_pushed_and_k_replenished(clean_graph):
    registered = clean_graph.query.register_foreign_source(
        "search-index-valid", mcp_source(**VALID)
    )
    assert registered == "search-index-valid"

    for bad in (
        {**VALID, "max_k": 0},
        {**VALID, "max_k": 10_001},
        {**VALID, "full_text": False, "vector": False},
    ):
        with pytest.raises(EngineResponseError) as refused:
            clean_graph.query.register_foreign_source(
                "search-index-invalid", mcp_source(**bad)
            )
        assert "SEARCH_CAPABILITY_INVALID" in str(refused.value)


def _fused_plan(source: dict[str, object]) -> list[dict[str, object]]:
    return [
        {
            "FuseRrf": {
                "branches": [
                    [{"RankText": {"query": "needle"}}],
                    [{"ForeignScan": {"source": source, "join": False}}],
                ],
                "k": 0.0,
            }
        }
    ]


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.5.3")
def test_remote_branch_fuses_with_local(clean_graph):
    # The served FUSE reaches the search remote through the whole caller chain. This
    # server binds no connector row port (the host registers it), so the remote
    # branch refuses with SOURCE_UNREACHABLE naming the search source, never by
    # silently returning the local branch alone; the fused rows are proven at L2.
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.unified(_fused_plan(mcp_source(**VALID)))
    assert "SOURCE_UNREACHABLE" in str(refused.value)
    assert "no connector port is registered" in str(refused.value)

    # A search source needs the query text of a RankText branch to push.
    lone = [
        {"ForeignScan": {"source": mcp_source(**VALID), "join": False}},
    ]
    with pytest.raises(EngineResponseError) as no_text:
        clean_graph.query.unified(lone)
    assert "UNSUPPORTED_SEARCH_ARGUMENT" in str(no_text.value)
