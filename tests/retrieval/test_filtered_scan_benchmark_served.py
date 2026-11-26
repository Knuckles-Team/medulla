"""Filter-aware scans return what a post-filter brute-force oracle returns
(MEDULLA-FEDERATED-QUERY-R040).

A selective hard constraint rides on ``UnifiedQuery.retrieval``. The vector rank and the
text rank each return the eligible rows in the order the unfiltered rank gives them,
and the rank of the full corpus filtered afterwards is the oracle. The latency ratio
at 1% selectivity is a nightly measurement and never gates a pull request.
"""

from __future__ import annotations

import math
import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

DOCS = 40
KEPT = 4
LIMIT = 100
_SCAN: dict[str, Any] = {"Scan": {"label": "Doc"}}


def _angle(index: int) -> float:
    return index * 0.07


def _kept(index: int) -> bool:
    return index % (DOCS // KEPT) == 3


def _ask(cl: Any, graph: str, ops: list[dict[str, Any]], constraints: Any) -> list[str]:
    body: dict[str, Any] = {"plan": {"ops": ops, "report_shortfall": True}}
    if constraints is not None:
        body["retrieval"] = {"constraints": constraints}
    answer = cl._send("UnifiedQuery", body, graph)
    if isinstance(answer, (bytes, bytearray)):
        answer = msgpack.unpackb(answer, raw=False)
    return [row[0] for row in answer["rows"]]


# proves R040: Filter-aware index scans for filtered vector and text search
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040")
def test_filtered_scan_matches_oracle(start_medulla_server) -> None:
    graph = f"filtered-scan-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        for index in range(DOCS):
            # Equal length text, differing term frequency: BM25 is monotone in it.
            frequency = index + 1
            text = " ".join(["needle"] * frequency + ["pad"] * (DOCS - frequency))
            put(
                cl,
                graph,
                f"d{index:02d}",
                {
                    "type": "Doc",
                    "kind": "doc" if _kept(index) else "memo",
                    "text": text,
                },
                [math.cos(_angle(index)), math.sin(_angle(index))],
            )
        constraints = {
            "graph_revision": 0,
            "entity_predicates": [
                {"operator": "eq", "property": "kind", "value": {"Str": "doc"}}
            ],
        }
        kept = {f"d{index:02d}" for index in range(DOCS) if _kept(index)}
        assert len(kept) == KEPT
        limit = {"Limit": {"k": LIMIT}}

        # Vector: the oracle is the cosine order over the eligible rows, which is also
        # the rank of the whole corpus with the other rows removed afterwards.
        rank = {"Rank": {"query": [1.0, 0.0]}}
        oracle = sorted(kept, key=lambda doc: _angle(int(doc[1:])))
        filtered = _ask(cl, graph, [_SCAN, rank, limit], constraints)
        assert filtered == oracle
        everything = _ask(cl, graph, [_SCAN, rank, limit], None)
        assert len(everything) == DOCS
        assert [doc for doc in everything if doc in kept] == oracle

        # Text: every row holds the term, so the eligible rows are exactly the kept
        # ones, in the order the unfiltered text rank gives them.
        text_rank = {"RankText": {"query": "needle"}}
        filtered_text = _ask(cl, graph, [_SCAN, text_rank, limit], constraints)
        unfiltered_text = _ask(cl, graph, [_SCAN, text_rank, limit], None)
        assert set(filtered_text) == kept
        assert filtered_text == [doc for doc in unfiltered_text if doc in kept]
    finally:
        cl.tenants.delete(graph)
        cl.close()
