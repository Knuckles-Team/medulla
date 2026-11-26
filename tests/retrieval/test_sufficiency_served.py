"""Served sufficiency judge over a retrieval round (MEDULLA-FEDERATED-QUERY-R081.6).

A plan returns two rows while the rule needs three. At round 1 of 3 the verdict is
`insufficient` with repair `boost_k` and `k` doubled, plus the `retrieval.sufficiency`
premise. At round 3 the repair is `stop`. A request without `round_limit` refuses with
`RETRIEVAL_ROUND_LIMIT_REQUIRED`, and a second tenant sees no rows.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, put, request, run_round

_SCAN = [{"Scan": {"label": "Doc"}}]


def _body(round_number: int, round_limit: int | None) -> dict[str, Any]:
    body = request(_SCAN)
    part = body["sufficiency"]
    part["round"] = round_number
    part["k"] = 4
    if round_limit is None:
        del part["round_limit"]
    else:
        part["round_limit"] = round_limit
    return body


# proves R081.6: Hot-path proof
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.6")
def test_insufficient_returns_boost_k(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.6."""
    graph = f"suff-{uuid.uuid4().hex[:8]}"
    other = f"suff-other-{uuid.uuid4().hex[:8]}"
    cl = client()
    try:
        cl.tenants.create(graph)
        cl.tenants.create(other)
        for node_id in ("doc-0", "doc-1"):
            put(cl, graph, node_id, {"type": "Doc", "text": "entry text"})

        first = run_round(cl, graph, _body(1, 3))
        assert len(first["rows"]) == 2
        verdict = first["sufficiency"]
        assert verdict["verdict"] == "insufficient"
        assert verdict["repair"] == {"kind": "boost_k", "k": 8}
        premises = [
            p for p in first["premises"] if p["name"] == "retrieval.sufficiency"
        ]
        assert len(premises) == 1, first["premises"]
        assert premises[0]["value"]["verdict"] == "insufficient"

        last = run_round(cl, graph, _body(3, 3))
        assert last["sufficiency"]["repair"]["kind"] == "stop"

        with pytest.raises(Exception) as missing:
            run_round(cl, graph, _body(1, None))
        assert "RETRIEVAL_ROUND_LIMIT_REQUIRED" in str(missing.value)

        # A second tenant holds none of those rows.
        assert run_round(cl, other, _body(1, 3))["rows"] == []
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
