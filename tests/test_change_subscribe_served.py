"""Committed user-table rows reach the change feed (MEDULLA-ENGINE-PARITY-R015.1).

A SQL write commits durable rows; the table store hands the committed row changes
to the change feed in commit order, and a subscriber reads them from a cursor,
resumes from the page's cursor and, once the retained window has moved past a
cursor, is refused `CURSOR_EXPIRED` with a cursor that reads again.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient


@pytest.fixture
def client():
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    c = SyncMedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    c.graph.clear()
    return c


def _rows(page, table):
    return [
        (event["change"]["operation"], event["change"]["key"])
        for event in page["events"]
        if event["change"]["kind"] == "row" and event["change"]["table"] == table
    ]


@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R015.1")
def test_cdc_read_resumes_and_expires(start_medulla_server, client):
    table = f"cdc_orders_{uuid.uuid4().hex[:8]}"
    client.query.sql(f"CREATE TABLE {table} (id int primary key, name text)")
    snapshot = client.graph.open_graph_snapshot()

    client.query.sql(f"INSERT INTO {table} VALUES (1, 'a'), (2, 'b')")
    client.query.sql(f"UPDATE {table} SET name = 'c' WHERE id = 2")
    client.query.sql(f"DELETE FROM {table} WHERE id = 1")

    # The committed rows arrive in commit order from the snapshot's cursor.
    page = client.graph.read_graph_changes(
        snapshot["high_water_cursor"], limit=50, table=table
    )
    assert _rows(page, table) == [
        ("insert", "1"),
        ("insert", "2"),
        ("update", "2"),
        ("delete", "1"),
    ]
    ordinals = [event["event_ordinal"] for event in page["events"]]
    assert ordinals == sorted(ordinals)

    # Another table's filter sees none of them, and its cursor still advances.
    other = client.graph.read_graph_changes(
        snapshot["high_water_cursor"], limit=50, table=f"{table}_other"
    )
    assert other["events"] == []
    assert other["next_cursor"] == page["next_cursor"]

    # Resuming from the page's cursor returns nothing seen, then only the new row.
    resume = page["next_cursor"]
    assert (
        client.graph.read_graph_changes(resume, limit=50, table=table)["events"] == []
    )
    client.query.sql(f"INSERT INTO {table} VALUES (3, 'd')")
    tail = client.graph.read_graph_changes(resume, limit=50, table=table)
    assert _rows(tail, table) == [("insert", "3")]

    # Commit more rows than the retained window holds: the old cursor is expired
    # and the refusal names a cursor that reads the retained rows.
    for start in range(10, 5_010, 1_000):
        values = ", ".join(f"({i}, 'x')" for i in range(start, start + 1_000))
        client.query.sql(f"INSERT INTO {table} VALUES {values}")
    with pytest.raises(EngineResponseError) as expired:
        client.graph.read_graph_changes(resume, limit=50, table=table)
    assert expired.value.code == "CURSOR_EXPIRED"
    safe = (expired.value.detail or "").split("safe_cursor=")[1].strip()
    retained = client.graph.read_graph_changes(safe, limit=50, table=table)
    assert retained["events"], "the retained window still reads"
    assert all(op == "insert" for op, _ in _rows(retained, table))
