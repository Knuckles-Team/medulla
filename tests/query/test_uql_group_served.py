"""The UQL GROUP stage over the served wire (MEDULLA-FEDERATED-QUERY-R733).

The browser graph views count nodes by label: one served ``Uql`` statement groups the
graph's nodes and returns one row per label with its count as a typed column.
"""

from __future__ import annotations

import uuid

import pytest

from medulla.client import EngineResponseError


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R733")
def test_node_type_counts_over_the_wire(clean_graph) -> None:
    tag = uuid.uuid4().hex[:8]
    doc, person = f"GroupDoc{tag}", f"GroupPerson{tag}"
    for index, (kind, size) in enumerate(
        [(doc, 10), (doc, 30), (doc, 20), (person, 70)]
    ):
        clean_graph.nodes.add(f"group-{tag}-{index}", {"type": kind, "size": size})

    result = clean_graph.query.uql(
        "MATCH () |> GROUP BY label AGGREGATE COUNT AS n, AVG(size) AS mean"
        " |> LIMIT 100000"
    )

    assert result["kind"] == "rows"
    assert result["columns"] == ["n", "mean"]
    by_label = {row["id"]: row["channels"] for row in result["rows"]}
    assert by_label[doc] == {"n": 3.0, "mean": 20.0}
    assert by_label[person] == {"n": 1.0, "mean": 70.0}

    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.uql("MATCH () |> GROUP BY label AGGREGATE SUM(no_such_field)")
    assert "UQL_UNKNOWN_FIELD" in str(refused.value)
