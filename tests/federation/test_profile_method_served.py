"""MEDULLA-UNIFIED-DATA-PLANE-R761.5: each column profile records its method and sample size.

A column of an attached PostgreSQL source is profiled by a pushed aggregate; a column of a
native table is profiled from a sample. Both job rows carry the method, the sampled row count
and the time the profile was taken.
"""

from __future__ import annotations

import time

import pytest


def _wait(client, job_id: str) -> dict:
    status: dict = {}
    for _ in range(300):
        status = client.jobs.status(job_id)
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            assert name == "Succeeded", f"profile job ended {state!r}"
            return status
        time.sleep(0.1)
    raise AssertionError(f"profile job did not finish: {status!r}")


def _profile(client, dataset: str) -> dict:
    graph = client._client._graph_name
    job = client.jobs.submit(
        graph, {"DatasetProfile": {"dataset": dataset, "columns": ["status"]}}
    )
    [row] = _wait(client, job["job_id"])["output"]["rows"]
    return row


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R761.5")
def test_profile_method_recorded(clean_graph, pg_source) -> None:
    pg_source.execute("DROP TABLE IF EXISTS prof_orders")
    pg_source.execute("CREATE TABLE prof_orders (id int primary key, status text)")
    pg_source.execute("INSERT INTO prof_orders VALUES (1,'open'),(2,'open'),(3,NULL)")
    clean_graph.query.register_foreign_source(
        "prof_src",
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": "SELECT id, status FROM prof_orders",
                "id_field": "id",
                "columns": ["status"],
            }
        },
    )
    clean_graph.query.sql("CREATE TABLE prof_native (id int primary key, status text)")
    clean_graph.query.sql(
        "INSERT INTO prof_native VALUES (1, 'open'), (2, 'open'), (3, NULL)"
    )

    attached = _profile(clean_graph, "prof_src.prof_orders")
    assert attached["method"] == "pushdown"
    assert attached["sampled_rows"] == 3
    assert attached["rows"] == 3
    assert isinstance(attached["profiled_at"], int) and attached["profiled_at"] > 0

    native = _profile(clean_graph, "prof_native")
    assert native["method"] == "sample"
    assert native["sampled_rows"] == 3
    assert isinstance(native["profiled_at"], int) and native["profiled_at"] > 0
