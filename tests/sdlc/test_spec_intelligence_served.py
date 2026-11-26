"""The spec intelligence report (MEDULLA-SDLC-R014.13 and R014.14).

L4: every call goes through the running engine. The report is the alternative body
``spec_report`` of ``UnifiedQuery``: it clusters the spec rows and the glossary
terms they use, reports the clusters that span repositories, and checks each row
against the row shapes of the spec standard. It carries no plan and writes nothing.
"""

from __future__ import annotations

import os
import uuid

import msgpack
import pytest
from conftest import request_context

from medulla.client import MedullaClient


def _node(node_id: str, node_type: str, **props: str) -> dict:
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {"type": node_type, **props},
    }


def _edge(source: str, relationship: str, target: str) -> dict:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


def _row(repo: str, row: str, status: str = "SPECIFIED") -> dict:
    return _node(
        f"requirement:{repo}:{row}",
        "Requirement",
        repo=repo,
        rowId=row,
        rowStatus=status,
    )


def _bound(repo: str, row: str) -> list[dict]:
    test = f"testfile:{repo}:tests/test_{row}.py"
    return [
        _node(test, "TestFile", repo=repo, path=f"tests/test_{row}.py"),
        _edge(f"requirement:{repo}:{row}", "boundTest", test),
    ]


def _terms(repo: str, row: str, *names: str) -> list[dict]:
    seed: list[dict] = []
    for name in names:
        seed.append(_node(f"term:{name}", "GlossaryTerm", name=name))
        seed.append(_edge(f"requirement:{repo}:{row}", "usesTerm", f"term:{name}"))
    return seed


def _ordered(seed: list[dict]) -> list[dict]:
    """Nodes before edges, so no edge names a node a later write adds."""
    return [op for op in seed if op["op"] == "upsert_node"] + [
        op for op in seed if op["op"] == "upsert_edge"
    ]


async def _with_graph(seed: list[dict], body):
    graph = f"sdlc-intel-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(_ordered(seed), use_bin_type=True)},
            graph,
        )
        try:
            return await body(client, graph)
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


def _overlap_seed() -> list[dict]:
    """Two rows of two repositories that write the same two terms, and one row of a
    third repository that shares nothing with them."""
    seed = [_row("a", "A-R001"), _row("b", "B-R001"), _row("c", "C-R001")]
    seed += _bound("a", "A-R001") + _bound("b", "B-R001") + _bound("c", "C-R001")
    seed += _terms("a", "A-R001", "UnifiedQuery", "row_id")
    seed += _terms("b", "B-R001", "UnifiedQuery", "row_id")
    seed += _terms("c", "C-R001", "ProjectMode")
    return seed


# spec: MEDULLA-SDLC-R014.13
@pytest.mark.spec("MEDULLA-SDLC-R014.13")
def test_overlap_pair_across_repositories_is_reported():
    import asyncio

    async def body(client, graph):
        return await client.spec_report()

    report = asyncio.run(_with_graph(_overlap_seed(), body))
    assert report.rows == 3
    assert report.violations == []
    assert len(report.overlaps) == 1
    overlap = report.overlaps[0]
    assert overlap.row_ids == ["A-R001", "B-R001"]
    assert overlap.repositories == ["a", "b"]
    assert overlap.shared_terms == ["UnifiedQuery", "row_id"]
    assert overlap.cluster_id.startswith("l1:")


# spec: MEDULLA-SDLC-R014.13
@pytest.mark.spec("MEDULLA-SDLC-R014.13")
def test_report_never_writes():
    import asyncio

    async def body(client, graph):
        before = await client._send(
            "GetNodesByLabel",
            {"label": "Requirement", "after": None, "limit": 100},
            graph,
        )
        # A report that carries a plan, or another alternative body, is refused.
        with pytest.raises(Exception, match="REPORT_READ_ONLY"):
            await client._send(
                "UnifiedQuery",
                {
                    "plan": {"ops": [{"Scan": {"label": "Requirement"}}]},
                    "spec_report": {},
                },
                graph,
            )
        # A resolution outside the range is refused before anything is read.
        with pytest.raises(Exception, match="SPEC_REPORT_INVALID"):
            await client.spec_report(resolution_micros=1)
        report = await client.spec_report(seed=7)
        after = await client._send(
            "GetNodesByLabel",
            {"label": "Requirement", "after": None, "limit": 100},
            graph,
        )
        return report, before, after

    report, before, after = asyncio.run(_with_graph(_overlap_seed(), body))
    assert report.rows == 3
    # The report wrote no node and no edge.
    assert sorted(entry[0] for entry in before) == sorted(entry[0] for entry in after)


def _journey_seed() -> list[dict]:
    """Three repositories. One requirement is duplicated across a and b (the same
    terms), one row is LANDED with no commit, and one row names a seam side on one
    side only. Every row has a bound test, so these are the only violations."""
    seed = [
        _row("a", "A-R001", "VERIFIED"),
        _row("a", "A-R002"),
        _row("b", "B-R001"),
        _row("c", "C-R001", "LANDED"),
        _row("c", "C-R002"),
    ]
    for repo, row in (
        ("a", "A-R001"),
        ("a", "A-R002"),
        ("b", "B-R001"),
        ("c", "C-R001"),
        ("c", "C-R002"),
    ):
        seed += _bound(repo, row)
    # A-R001 is verified: a commit implements it and a test verifies it.
    seed.append(
        _node("changeset:a:" + "c" * 8, "Changeset", repo="a", changeset_id="c" * 8)
    )
    seed.append(
        _edge("requirement:a:A-R001", "implementedByCommit", "changeset:a:" + "c" * 8)
    )
    seed.append(
        _edge("requirement:a:A-R001", "verifiedBy", "testfile:a:tests/test_A-R001.py")
    )
    seed.append(_edge("requirement:a:A-R002", "dependsOnRow", "requirement:a:A-R001"))
    # The duplicate pair of repositories a and b.
    seed += _terms("a", "A-R001", "UnifiedQuery", "row_id")
    seed += _terms("b", "B-R001", "UnifiedQuery", "row_id")
    # C-R002 names the provider side of a seam and not the consumer side.
    seed.append(
        _node("seam:c:contracts/x.md", "SeamSide", repo="c", path="contracts/x.md")
    )
    seed.append(_edge("requirement:c:C-R002", "seamProvider", "seam:c:contracts/x.md"))
    return seed


# spec: MEDULLA-SDLC-R014.14
@pytest.mark.spec("MEDULLA-SDLC-R014.14")
def test_fixture_journey_ingests_queries_and_reports():
    import asyncio

    async def body(client, graph):
        # The ordinary UnifiedQuery returns row to commit and row to depends-on row.
        commits = await client.query.unified(
            [
                {"Scan": {"label": "Requirement"}},
                {"Traverse": {"rel": "implementedByCommit", "min": 1, "max": 1}},
            ]
        )
        depends = await client.query.unified(
            [
                {"Scan": {"label": "Requirement"}},
                {"Traverse": {"rel": "dependsOnRow", "min": 1, "max": 1}},
            ]
        )
        return commits, depends, await client.spec_report()

    commits, depends, report = asyncio.run(_with_graph(_journey_seed(), body))
    assert [row["id"] for row in commits] == ["changeset:a:" + "c" * 8]
    assert [row["id"] for row in depends] == ["requirement:a:A-R001"]
    assert report.rows == 5
    overlaps = [(o.row_ids, o.repositories) for o in report.overlaps]
    assert overlaps == [(["A-R001", "B-R001"], ["a", "b"])]
    # spec: MEDULLA-SDLC-R014.14
    assert [(v.row_id, v.code) for v in report.violations] == [
        ("C-R001", "LANDED_WITHOUT_COMMIT"),
        ("C-R002", "SEAM_SIDE_MISSING"),
    ]
