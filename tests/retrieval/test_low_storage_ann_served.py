"""Low-storage ANN selectivity path, served (MEDULLA-FEDERATED-QUERY-R040.5, R040.5.6).

A selective top-`k` vector rank through authenticated `UnifiedQuery` visits at most
`4 * result_rows + 64` rows at corpus sizes 1000, 10000 and 100000. The receipt of the
answer reports the rows visited (`examined`), whether the index holds no tombstones
(`compacted`) and whether the bound was crossed (`visit_bound_hit`). The rows are
compared with the exact full-scan oracle computed from the known vectors.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

from medulla.client import EngineResponseError

K = 10
BOUND = 4 * K + 64
# Corpus size -> the number of nodes the rank is allowed to see. Up to 4096 eligible
# candidates the rank is an exact scan, so its selection must be no larger than the
# bound; above that it is the filtered walk, whose window is `4 * k` rows however large
# the corpus.
SELECTED = {1000: 100, 10000: 5000, 100000: 5000}


def _vector(index: int) -> list[float]:
    """Nearest to the query `[1, 0]` is the smallest index."""
    return [1.0, index * 0.001]


def _rank(cl: Any, graph: str, k: int) -> dict[str, Any]:
    ops = [
        {"Scan": {"label": "Doc"}},
        {"Rank": {"query": [1.0, 0.0]}},
        {"Limit": {"k": k}},
    ]
    result = cl._send(
        "UnifiedQuery", {"plan": {"ops": ops, "report_shortfall": True}}, graph
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _corpus(cl: Any, graph: str, size: int) -> None:
    """`size` embedded nodes: the first `SELECTED[size]` are `Doc`, the rest `Other`."""
    selected = SELECTED[size]
    for index in range(size):
        label = "Doc" if index < selected else "Other"
        put(cl, graph, f"n{index:06d}", {"type": label}, _vector(index))


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.5")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.5.6")
@pytest.mark.parametrize("size", sorted(SELECTED))
def test_selectivity_bound_and_readback(start_medulla_server, size: int) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.5.6: a selective top-k rank visits at
    most `4 * result_rows + 64` rows at the corpus size, returns the exact full-scan
    rows, and reads back its receipt through the served answer."""
    graph = f"selectivity-{size}-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _corpus(cl, graph, size)

        answer = _rank(cl, graph, K)
        # The exact oracle: the K selected nodes nearest the query, in order.
        oracle = [f"n{index:06d}" for index in range(K)]
        assert [row[0] for row in answer["rows"]] == oracle
        assert answer.get("shortfall") is None

        ann = answer["receipt"]["ann"]
        assert ann["examined"] <= BOUND, (size, ann)
        assert ann["visit_bound_hit"] is False, (size, ann)
        assert isinstance(ann["compacted"], bool)

        # An update changes the answer and the receipt still holds the bound: the
        # nearest node leaves the graph, the next one takes its place at the end of
        # the top-k.
        cl._send("RemoveNode", {"node_id": oracle[0]}, graph)
        updated = _rank(cl, graph, K)
        assert [row[0] for row in updated["rows"]] == [
            f"n{index:06d}" for index in range(1, K + 1)
        ]
        assert updated["receipt"]["ann"]["examined"] <= BOUND
        assert updated["receipt"]["ann"]["visit_bound_hit"] is False

        # A rank of zero rows is refused before any row is read.
        with pytest.raises(EngineResponseError) as refused:
            _rank(cl, graph, 0)
        assert "RETRIEVAL_K_INVALID" in str(refused.value)
    finally:
        cl.tenants.delete(graph)
        cl.close()
