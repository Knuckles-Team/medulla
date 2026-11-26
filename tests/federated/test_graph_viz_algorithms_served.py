"""Served acceptance of the graph-visualization reads (FEDERATED-QUERY-R743, R744).

``CommunityDetection``, ``PageRank`` and ``GetShortestPath`` take an optional
``node_ids`` scope, limited to the caller's visible set, and answer typed views. A
node the caller cannot see answers byte-identically to an unknown one. The cluster
hierarchy reads answer the published ``ClusterLevelView`` and ``ClusterExpansion``.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import request_context, tenant_request_context

from medulla.client import MedullaClient
from medulla.generated.models import (
    AlgorithmScopeView,
    ClusterExpansion,
    ClusterHierarchySummary,
    ClusterLevelView,
    CommunityDetectionView,
    PageRankView,
    ShortestPathView,
)

CHAIN = ["a", "b", "c", "d"]


async def _connect(graph: str, context: dict[str, object]) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=graph,
    )


async def _seed_chain(client: MedullaClient) -> None:
    for node in CHAIN:
        await client.nodes.add(node, {"type": "Doc"})
    for source, target in zip(CHAIN, CHAIN[1:], strict=False):
        await client.edges.add(source, target, {"relationship": "LINKS"})


async def _owner_graph() -> tuple[str, MedullaClient]:
    graph = f"viz-{uuid.uuid4().hex[:8]}"
    admin = await _connect("__commons__", request_context())
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    owner = await _connect(graph, request_context())
    await _seed_chain(owner)
    return graph, owner


async def _drop(graph: str, owner: MedullaClient) -> None:
    try:
        await owner.tenants.delete(graph)
    finally:
        await owner.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743.5")
def test_python_client_marshals_node_ids_and_decodes_the_typed_views() -> None:
    async def exercise() -> None:
        graph, owner = await _owner_graph()
        try:
            unscoped = await owner.analytics.pagerank(damping=0.85, iterations=20)
            assert isinstance(unscoped, PageRankView)
            assert unscoped.scope is None
            assert sorted(row.node_id for row in unscoped.scores) == CHAIN

            scoped = await owner.analytics.pagerank(node_ids=["a", "b", "c"])
            assert sorted(row.node_id for row in scoped.scores) == ["a", "b", "c"]
            assert scoped.scope == AlgorithmScopeView(requested=3, in_scope=3)

            path = await owner.graph.shortest_path("a", "d")
            assert isinstance(path, ShortestPathView)
            assert path.path == CHAIN and path.scope is None
            cut = await owner.graph.shortest_path("a", "d", node_ids=["a", "b", "d"])
            assert cut.path is None
            assert cut.scope == AlgorithmScopeView(requested=3, in_scope=3)

            communities = await owner.graph.community_detection(node_ids=["a", "b"])
            assert isinstance(communities, CommunityDetectionView)
            assert sorted(sum(communities.communities, [])) == ["a", "b"]
        finally:
            await _drop(graph, owner)

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743.6")
def test_a_hidden_node_answers_byte_identically_to_an_unknown_one() -> None:
    async def exercise() -> None:
        graph, owner = await _owner_graph()
        foreign = await _connect(graph, tenant_request_context("tenant:foreign"))
        try:
            hidden = ["a", "b"]
            unknown = [
                f"absent-{uuid.uuid4().hex[:6]}",
                f"absent-{uuid.uuid4().hex[:6]}",
            ]
            seen = await owner.analytics.pagerank(node_ids=hidden)
            assert seen.scope == AlgorithmScopeView(requested=2, in_scope=2)

            for request in (
                lambda ids: foreign.analytics.pagerank(node_ids=ids),
                lambda ids: foreign.graph.shortest_path(ids[0], ids[1], node_ids=ids),
                lambda ids: foreign.graph.community_detection(node_ids=ids),
            ):
                from_hidden = await request(hidden)
                from_unknown = await request(unknown)
                assert from_hidden.model_dump_json() == from_unknown.model_dump_json()
                assert from_hidden.scope == AlgorithmScopeView(requested=2, in_scope=0)
        finally:
            await foreign.close()
            await _drop(graph, owner)

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R743.4")
def test_a_scope_above_the_batch_bound_is_refused() -> None:
    async def exercise() -> None:
        graph, owner = await _owner_graph()
        try:
            with pytest.raises(Exception, match="INVALID_ARGUMENT"):
                await owner.analytics.pagerank(
                    node_ids=[f"n{index}" for index in range(100_001)]
                )
        finally:
            await _drop(graph, owner)

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R744")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R744.3")
def test_cluster_hierarchy_reads_answer_the_published_views() -> None:
    async def exercise() -> None:
        graph, owner = await _owner_graph()
        try:
            summary = await owner.graph.cluster_hierarchy_refresh()
            assert isinstance(summary, ClusterHierarchySummary)
            assert summary.base_node_count == len(CHAIN)

            level = await owner.graph.cluster_hierarchy_clusters(level=1)
            assert isinstance(level, ClusterLevelView)
            assert level.level == 1
            assert sum(cluster.node_count for cluster in level.clusters) == len(CHAIN)

            expansion = await owner.graph.cluster_hierarchy_expand(level.clusters[0].id)
            assert isinstance(expansion, ClusterExpansion)
            assert expansion.child_clusters == []
            assert {node.id for node in expansion.nodes} <= set(CHAIN)
        finally:
            await _drop(graph, owner)

    asyncio.run(exercise())
