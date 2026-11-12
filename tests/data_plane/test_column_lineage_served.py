"""Served column lineage (MEDULLA-UNIFIED-DATA-PLANE-R763.2 to R763.6).

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


def _live_postgres() -> tuple[str, Any]:
    """The nightly lane's PostgreSQL server (R013.1), or a skip."""
    dsn = os.environ.get("MEDULLA_TEST_PG_DSN")
    psycopg = pytest.importorskip("psycopg")
    if not dsn:
        pytest.skip(
            "MEDULLA_TEST_PG_DSN is not set: the nightly lane runs the live source"
        )
    return dsn, psycopg


def _pg_exec(dsn: str, psycopg: Any, statements: list[str]) -> None:
    with psycopg.connect(dsn, autocommit=True) as conn:
        for statement in statements:
            conn.execute(statement)


def _attach(client: Any, name: str, dsn: str) -> None:
    client.query.register_foreign_source(
        name, {"Sql": {"dsn": dsn, "query": "SELECT 1 AS id", "id_field": "id"}}
    )


def _from_catalog(client: Any, graph: str, source: str) -> dict:
    """DeriveFromCatalog, retried while the registration probe is still reading."""
    import time

    deadline = time.monotonic() + 60
    while True:
        try:
            op = {"op": "derive_from_catalog", "source": source}
            return _decode(client._send("ColumnLineage", {"op": op}, graph))
        except Exception as error:
            if (
                "LINEAGE_CATALOG_MISSING" not in str(error)
                or time.monotonic() > deadline
            ):
                raise
            time.sleep(1)


def _impact(
    client: Any, graph: str, column: str, direction: str, depth: int | None = None
) -> dict:
    op = {"op": "impact", "column": column, "direction": direction, "max_depth": depth}
    return _decode(client._send("ColumnLineage", {"op": op}, graph))


def _drain(client: Any, graph: str, events: list[dict]) -> dict:
    return _decode(client._send("LineageIngest", {"events": events}, graph))


def _sql_event(event_id: str, sql: str, target: str, source: str) -> dict:
    return {
        "event_id": event_id,
        "run": {
            "run_id": event_id,
            "job": "views",
            "started_at_ms": 1,
            "ended_at_ms": 2,
        },
        "inputs": [{"namespace": "pg", "name": "public.orders"}],
        "outputs": [{"namespace": "pg", "name": target}],
        "sql_text": sql,
        "dialect": "postgres",
        "catalog_source": source,
    }


_VIEW = (
    "CREATE OR REPLACE VIEW public.big_orders AS "
    "SELECT o.id, o.amount * 2 AS doubled FROM public.orders o WHERE o.amount > 100"
)
_TABLE = "CREATE TABLE IF NOT EXISTS public.orders (id int primary key, amount int)"


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.2")
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.3")
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.7")
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

        # No stored catalog and no edges: the new operations refuse or stay empty.
        op = {"op": "derive_from_catalog", "source": "nowhere"}
        with pytest.raises(Exception, match="LINEAGE_CATALOG_MISSING"):
            client._send("ColumnLineage", {"op": op}, graph)
        assert (
            _impact(client, graph, "public.orders.amount", "downstream")["edges"] == []
        )
        with pytest.raises(Exception, match="LINEAGE_DEPTH_INVALID"):
            _impact(client, graph, "public.orders.amount", "downstream", 0)
        with pytest.raises(Exception, match="LINEAGE_DEPTH_INVALID"):
            _impact(client, graph, "public.orders.amount", "downstream", 33)

        # An attached source with a view: the held catalog derives the view's edges, the
        # drain stores them, and the impact walk answers; tenant B sees nothing.
        dsn, psycopg = _live_postgres()
        _pg_exec(dsn, psycopg, [_TABLE, _VIEW])
        _attach(client, "lineage-pg", dsn)
        derived = _from_catalog(client, graph, "lineage-pg")
        assert (
            _transform(derived, "public.orders.amount", "public.big_orders.doubled")
            == "expression"
        )
        receipt = _drain(
            client,
            graph,
            [
                _sql_event(
                    "v1", _VIEW.split(" AS ", 1)[1], "public.big_orders", "lineage-pg"
                )
            ],
        )
        assert receipt["column_edges"] >= 2
        down = _impact(client, graph, "public.orders.amount", "downstream")
        assert (
            _transform(down, "public.orders.amount", "public.big_orders.doubled")
            == "expression"
        )
        assert (
            _impact(client, graph, "public.orders.amount", "downstream", 1)["edges"]
            == down["edges"]
        )
        up = _impact(client, graph, "public.big_orders.doubled", "upstream")
        assert (
            _transform(up, "public.orders.amount", "public.big_orders.doubled")
            == "expression"
        )
        other = f"{graph}-b"
        client.tenants.create(other)
        try:
            assert (
                _impact(client, other, "public.orders.amount", "downstream")["edges"]
                == []
            )
        finally:
            client.tenants.delete(other)
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.4")
def test_unparseable_sql_quarantines(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R763.4: a bad statement quarantines."""
    dsn, psycopg = _live_postgres()
    graph = "column-lineage-quarantine"
    client = _client()
    client.tenants.create(graph)
    try:
        _pg_exec(dsn, psycopg, [_TABLE])
        _attach(client, "lineage-pg-q", dsn)
        _from_catalog(client, graph, "lineage-pg-q")
        bad = "SELEC id FROM public.orders"
        good = "SELECT id FROM public.orders"
        receipt = _drain(
            client,
            graph,
            [
                _sql_event("bad", bad, "public.v_bad", "lineage-pg-q"),
                _sql_event("good", good, "public.v_good", "lineage-pg-q"),
            ],
        )
        import hashlib

        assert receipt["quarantined"] == 1
        record = receipt["quarantine"][0]
        assert record["payload_digest"] == hashlib.sha256(bad.encode()).hexdigest()
        assert record["reason"]["kind"] == "sql_unparseable"
        assert receipt["column_edges"] == 1, "the good statement still derives"
        down = _impact(client, graph, "public.orders.id", "downstream")
        assert [e["target_column"] for e in down["edges"]] == ["public.v_good.id"]
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.5")
def test_edge_evidence_reads_back(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R763.5: evidence reads back."""
    dsn, psycopg = _live_postgres()
    graph = "column-lineage-evidence"
    client = _client()
    client.tenants.create(graph)
    try:
        _pg_exec(dsn, psycopg, [_TABLE])
        _attach(client, "lineage-pg-e", dsn)
        _from_catalog(client, graph, "lineage-pg-e")
        sql = "SELECT id FROM public.orders"
        _drain(client, graph, [_sql_event("e1", sql, "public.v_e", "lineage-pg-e")])
        first = _impact(client, graph, "public.orders.id", "downstream")["edges"]
        assert len(first) == 1
        edge = first[0]
        assert edge["parser_version"] == "sqlparser-0.62"
        assert len(edge["query_digest"]) == 64
        assert edge["transform"] == "identity"
        # The same text re-spaced reproduces the digest and adds no edge.
        again = _drain(
            client,
            graph,
            [
                _sql_event(
                    "e2",
                    "SELECT   id\n FROM public.orders",
                    "public.v_e",
                    "lineage-pg-e",
                )
            ],
        )
        assert again["column_edges"] == 0
        assert (
            _impact(client, graph, "public.orders.id", "downstream")["edges"] == first
        )
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R763.6")
def test_thousand_views_derive(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R763.6: 1,000 views, idempotent."""
    dsn, psycopg = _live_postgres()
    graph = "column-lineage-thousand"
    client = _client()
    client.tenants.create(graph)
    try:
        _pg_exec(dsn, psycopg, [_TABLE])
        _attach(client, "lineage-pg-k", dsn)
        _from_catalog(client, graph, "lineage-pg-k")
        events = [
            _sql_event(
                f"k{i}",
                f"SELECT id, amount + {i} AS a FROM public.orders",
                f"public.kv_{i}",
                "lineage-pg-k",
            )
            for i in range(1000)
        ]
        receipt = _drain(client, graph, events)
        assert receipt["column_edges"] == 2000
        replay = _drain(client, graph, events)
        assert replay["column_edges"] == 0, "a replay adds no edge"
    finally:
        client.tenants.delete(graph)
        client.close()
