"""A registered outside PostgreSQL source takes the join filter (MEDULLA-ENGINE-PARITY-R019.3)."""

from __future__ import annotations

import re

import psycopg
import pytest

ENGINE_ROWS = 100_000
OUTSIDE_ROWS = 10_000
SELECTED_ROWS = 100
SOURCE = "ext_join_source"


def _seed_outside(dsn: str) -> None:
    """`ext(id, k)`: ten thousand rows of which exactly one hundred carry `k = 'hit'`."""
    with psycopg.connect(dsn, autocommit=True) as connection:
        connection.execute("DROP TABLE IF EXISTS ext")
        connection.execute("CREATE TABLE ext (id text PRIMARY KEY, k text NOT NULL)")
        with connection.cursor() as cursor:
            with cursor.copy("COPY ext (id, k) FROM STDIN") as copy:
                for index in range(OUTSIDE_ROWS):
                    key = "hit" if index < SELECTED_ROWS else "miss"
                    copy.write_row((f"row:{index}", key))


def _stage_clauses(report: dict) -> list[str]:
    return [stage["clause"] for stage in report["stages"]]


# spec: MEDULLA-ENGINE-PARITY-R019.3
@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R019.3")
def test_federated_join_pushes_down(clean_graph, outside_postgres_dsn):
    _seed_outside(outside_postgres_dsn)
    for index in range(ENGINE_ROWS):
        clean_graph.nodes.add(f"row:{index}", {"type": "Row", "ordinal": index})
    clean_graph.register_foreign_source(
        SOURCE,
        {
            "Sql": {
                "dsn": outside_postgres_dsn,
                "query": "SELECT id, k FROM ext",
                "id_field": "id",
                "score_field": "k",
            }
        },
    )

    report = clean_graph.uql(
        f"PROFILE MATCH (:Row) |> FOREIGN SCAN '{SOURCE}' JOIN |> WHERE k = 'hit' "
        "|> RETURN id"
    )

    rows = report["rows"]
    assert sorted(row["id"] for row in rows) == [f"row:{i}" for i in range(SELECTED_ROWS)]
    remote = [clause for clause in _stage_clauses(report) if clause.startswith("REMOTE ")]
    assert remote, "the profile has no foreign stage"
    fetched = re.search(r"fetched=(\d+)", remote[0])
    assert fetched is not None, remote[0]
    # Only a filter that ran in the outside database fetches 100 rows; an unfiltered
    # fetch reports 10000.
    assert int(fetched.group(1)) == SELECTED_ROWS, remote[0]
