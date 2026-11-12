"""Served column lineage (MEDULLA-UNIFIED-DATA-PLANE-R763.2).

`ColumnLineage` derives column edges from one SQL query against a catalog on the
shared parser. A bare column is `identity`, an aggregate is `aggregate`, any other
expression is `expression`, and a column used only in a filter gives a
`filter_only` edge to `<target>.*`. A column that cannot be resolved is a typed
gap, never an edge.
"""

from __future__ import annotations

import os
from typing import Any

import msgpack
import pytest
from conftest import request_context

_CATALOG = [
    {
        "schema": "public",
        "name": "orders",
        "columns": [
            {"name": "id"},
            {"name": "customer_id"},
            {"name": "amount"},
            {"name": "status"},
        ],
    },
    {
        "schema": "public",
        "name": "customers",
        "columns": [{"name": "id"}, {"name": "name"}, {"name": "region"}],
    },
]

_VIEW_SQL = (
    "SELECT c.name AS customer, SUM(o.amount) AS total, o.amount * 2 AS doubled "
    "FROM public.orders o JOIN public.customers c ON c.id = o.customer_id "
    "WHERE c.region = 'eu' GROUP BY c.name"
)


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _derive(client: Any, graph: str, sql: str, target: str = "v") -> dict:
    op = {
        "op": "derive_from_sql",
        "sql": sql,
        "dialect": "postgres",
        "target": target,
        "catalog": _CATALOG,
    }
    return _decode(client._send("ColumnLineage", {"op": op}, graph))


def _transform(result: dict, source: str, target: str) -> str | None:
    for edge in result["edges"]:
        if edge["source_column"] == source and edge["target_column"] == target:
            return edge["transform"]
    return None


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.2")
def test_view_lineage_and_impact(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R763.2: every transform, served."""
    graph = "column-lineage-served"
    client = _client()
    client.tenants.create(graph)
    try:
        result = _derive(client, graph, _VIEW_SQL)
        assert _transform(result, "public.customers.name", "v.customer") == "identity"
        assert _transform(result, "public.orders.amount", "v.total") == "aggregate"
        assert _transform(result, "public.orders.amount", "v.doubled") == "expression"
        assert _transform(result, "public.customers.region", "v.*") == "filter_only"
        assert result["gaps"] == []
        digests = {edge["query_digest"] for edge in result["edges"]}
        assert len(digests) == 1

        # A column the catalog does not hold is a typed gap and never an edge.
        gapped = _derive(client, graph, "SELECT nope AS missing FROM public.orders")
        assert gapped["edges"] == []
        assert [g["output"] for g in gapped["gaps"]] == ["v.missing"]

        # A statement that is not a query is refused.
        with pytest.raises(Exception, match="LINEAGE_SQL_NOT_A_QUERY"):
            _derive(client, graph, "DELETE FROM public.orders")
    finally:
        client.tenants.delete(graph)
        client.close()
