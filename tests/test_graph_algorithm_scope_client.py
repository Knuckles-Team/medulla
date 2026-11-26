"""The Python mirror of the scoped graph algorithms.

``GraphOperationsClient.shortest_path`` / ``community_detection`` and
``AnalyticsClient.pagerank`` send ``node_ids`` only when the caller gives one and
decode the typed views of MEDULLA-FEDERATED-QUERY-R743. No engine is needed: a fake
client records what is sent.
"""

from __future__ import annotations

from typing import Any

import pytest

from medulla.client import AnalyticsClient, GraphOperationsClient, MedullaClient
from medulla.generated.models import (
    AlgorithmScopeView,
    CommunityDetectionView,
    PageRankView,
    ShortestPathView,
)

pytestmark = pytest.mark.no_engine


class _FakeClient(MedullaClient):
    def __init__(self, ret: Any = None) -> None:
        self.sent: list[tuple[str, dict[str, Any] | None]] = []
        self._ret = ret

    async def _send(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        graph: str | None = None,
        *,
        idempotency_key: str | None = None,
    ) -> Any:
        self.sent.append((method, params))
        return self._ret


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743.5")
@pytest.mark.asyncio
async def test_pagerank_sends_node_ids_only_when_scoped_and_decodes_the_view() -> None:
    fake = _FakeClient(
        ret={
            "scores": [{"node_id": "a", "score": 0.6}, {"node_id": "b", "score": 0.4}],
            "scope": {"requested": 3, "in_scope": 2},
        }
    )
    analytics = AnalyticsClient(fake)
    view = await analytics.pagerank(damping=0.5, iterations=7, node_ids=["a", "b", "z"])
    assert fake.sent == [
        ("PageRank", {"damping": 0.5, "iterations": 7, "node_ids": ["a", "b", "z"]})
    ]
    assert isinstance(view, PageRankView)
    assert view.scope == AlgorithmScopeView(requested=3, in_scope=2)
    assert [row.node_id for row in view.scores] == ["a", "b"]

    unscoped = _FakeClient(ret={"scores": []})
    view = await AnalyticsClient(unscoped).pagerank()
    assert unscoped.sent == [("PageRank", {"damping": 0.85, "iterations": 100})]
    assert view.scope is None


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743.5")
@pytest.mark.asyncio
async def test_shortest_path_and_communities_decode_their_typed_views() -> None:
    fake = _FakeClient(ret={"path": ["a", "b"], "scope": None})
    graph = GraphOperationsClient(fake)
    path = await graph.shortest_path("a", "b")
    assert fake.sent == [("GetShortestPath", {"source_id": "a", "target_id": "b"})]
    assert isinstance(path, ShortestPathView) and path.path == ["a", "b"]

    fake = _FakeClient(ret={"path": None, "scope": {"requested": 1, "in_scope": 0}})
    cut = await GraphOperationsClient(fake).shortest_path("a", "b", node_ids=["a"])
    assert fake.sent == [
        ("GetShortestPath", {"source_id": "a", "target_id": "b", "node_ids": ["a"]})
    ]
    assert cut.path is None and cut.scope == AlgorithmScopeView(requested=1, in_scope=0)

    fake = _FakeClient(ret={"communities": [["a", "b"], ["c"]]})
    partition = await GraphOperationsClient(fake).community_detection(resolution=2.0)
    assert fake.sent == [("CommunityDetection", {"resolution": 2.0})]
    assert isinstance(partition, CommunityDetectionView)
    assert partition.communities == [["a", "b"], ["c"]]
