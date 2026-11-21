"""Served `SchemaContext` (MEDULLA-UNIFIED-DATA-PLANE-R029.2).

One table of an attached source answers by intent: its columns (`definition`), the joins
that reach it (`joins`), its ontology class (`ontology`), its owner (`owner`) and what a
change reaches (`impact`). An unknown table and a source the caller did not register are
refused; an unmapped table never answers a default class.
"""

from __future__ import annotations

import os
import time
from typing import Any

import msgpack
import pytest
from conftest import request_context

_TABLES = [
    "CREATE TABLE IF NOT EXISTS public.sc_customers (id int primary key, name text)",
    "CREATE TABLE IF NOT EXISTS public.sc_orders ("
    "id int primary key, amount int, customer_id int references public.sc_customers (id))",
    "CREATE OR REPLACE VIEW public.sc_big_orders AS "
    "SELECT o.id, o.amount * 2 AS doubled FROM public.sc_orders o WHERE o.amount > 100",
]


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


def _context(
    client: Any, graph: str, source: str, table: str, intent: str, schema: str = "public"
) -> dict:
    params = {
        "source_id": source,
        "schema": schema,
        "object": table,
        "intent": intent,
    }
    return _decode(client._send("SchemaContext", params, graph))


def _context_when_probed(
    client: Any, graph: str, source: str, table: str, intent: str
) -> dict:
    """SchemaContext, retried while the registration probe is still reading."""
    deadline = time.monotonic() + 60
    while True:
        try:
            return _context(client, graph, source, table, intent)
        except Exception as error:
            if "SOURCE_UNAVAILABLE" not in str(error) or time.monotonic() > deadline:
                raise
            time.sleep(1)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R029.2")
def test_schema_context_answers_each_intent_and_refuses_unknown_table(
    start_medulla_server,
) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R029.2: joins, class, owner and impact, served."""
    dsn = os.environ.get("MEDULLA_TEST_PG_DSN")
    psycopg = pytest.importorskip("psycopg")
    if not dsn:
        pytest.skip(
            "MEDULLA_TEST_PG_DSN is not set: the nightly lane runs the live source"
        )
    graph = "schema-context-served"
    source = "schema-context-pg"
    client = _client()
    client.tenants.create(graph)
    try:
        with psycopg.connect(dsn, autocommit=True) as conn:
            for statement in _TABLES:
                conn.execute(statement)
        client.query.register_foreign_source(
            source, {"Sql": {"dsn": dsn, "query": "SELECT 1 AS id", "id_field": "id"}}
        )

        definition = _context_when_probed(client, graph, source, "sc_orders", "definition")
        assert {c["name"] for c in definition["columns"]} >= {
            "id",
            "amount",
            "customer_id",
        }
        assert definition["application"] == source

        # The foreign key is a join from both sides.
        joins = _context(client, graph, source, "sc_orders", "joins")["joins"]
        assert any(
            j["joined_table"] == "public.sc_customers" and j["on_column"] == "customer_id"
            for j in joins
        )
        reverse = _context(client, graph, source, "sc_customers", "joins")["joins"]
        assert any(
            j["joined_table"] == "public.sc_orders" and j["joined_column"] == "customer_id"
            for j in reverse
        )

        # The owner is the agent that registered the source.
        assert _context(client, graph, source, "sc_orders", "owner")["owner"]

        # No registered graph view maps the table: refused, never a default class.
        with pytest.raises(Exception, match="SCHEMA_CONTEXT_UNMAPPED"):
            _context(client, graph, source, "sc_orders", "ontology")

        # Stored column edges give the impact of a change.
        client._send(
            "LineageIngest",
            {
                "events": [
                    {
                        "event_id": "sc-1",
                        "run": {
                            "run_id": "sc-1",
                            "job": "views",
                            "started_at_ms": 1,
                            "ended_at_ms": 2,
                        },
                        "inputs": [{"namespace": "pg", "name": "public.sc_orders"}],
                        "outputs": [{"namespace": "pg", "name": "public.sc_big_orders"}],
                        "sql_text": _TABLES[2].split(" AS ", 1)[1],
                        "dialect": "postgres",
                        "catalog_source": source,
                    }
                ]
            },
            graph,
        )
        impact = _context(client, graph, source, "sc_orders", "impact")
        assert "public.sc_big_orders.doubled" in impact["change_impact"]

        # An unknown table and an unregistered source are refused, not defaulted.
        with pytest.raises(Exception, match="SCHEMA_CONTEXT_TABLE_UNKNOWN"):
            _context(client, graph, source, "no_such_table", "definition")
        with pytest.raises(Exception, match="SCHEMA_CONTEXT_SOURCE_UNKNOWN"):
            _context(client, graph, "nowhere", "sc_orders", "definition")
    finally:
        client.tenants.delete(graph)
        client.close()
