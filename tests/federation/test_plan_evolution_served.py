"""MEDULLA-FEDERATED-QUERY-R091.3 served acceptance: a kept plan is read by a later
query.

A served query over a registered foreign source records one observation of its shape
under the verified owner. The downtime tick submits the ``plan_evolution`` job, the
engine runs it in process (replaying the baseline and the candidate plan against the
source), keeps the candidate that fetches fewer rows with the same results, and
completes the job. A later join over the same source then reads the kept plan: its
PROFILE fragment names the kept digest."""

from __future__ import annotations

import os
import time
import uuid
from typing import Any

import pytest
from conftest import tenant_request_context

from medulla.client import SyncMedullaClient

# The knobs resolve from the environment of the server process, which the session
# fixture starts from this process's environment: name them at collection time.
os.environ.setdefault("MEDULLA_DOWNTIME_TICK_SECS", "1")
os.environ.setdefault("MEDULLA_DOWNTIME_IDLE_QUIET_SECS", "10")
os.environ.setdefault("MEDULLA_FEDERATION_MIN_REPLAYS", "5")

DW = "https://knuckles-team.github.io/ontology/downtime-work#"
ROWS = 50
LOCAL_IDS = 8


def _scan(name: str, join: bool) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": join}}


def _provision_downtime(graph) -> None:
    """One idle ``plan_evolution`` job type and the two capacity cells it leases."""
    graph.nodes.add(
        "dw:plan-evolution",
        {
            "type": "JobType",
            "intent": "plan_evolution",
            "priority": 5,
            "windowKind": "idle",
        },
    )
    graph.nodes.add(
        "dw:plan-evolution-budget",
        {"type": "Budget", "cpuMs": 1000, "memoryBytes": 1048576},
    )
    graph.edges.add(
        "dw:plan-evolution", "dw:plan-evolution-budget", {"relationship": "hasBudget"}
    )
    for cell_id, resource_class, capacity in (
        ("downtime:cpu", "cpu", 1_000_000),
        ("downtime:memory", "memory", 1 << 32),
    ):
        graph.capacity_leases.update_cell(
            {
                "schema_version": "1",
                "cell": {
                    "cell_id": cell_id,
                    "resource_class": resource_class,
                    "capacity": capacity,
                    "reserved_floor": 0,
                    "epoch": 1,
                    "policy_digest": "plan-evolution-served",
                    "updated_at_ms": int(time.time() * 1000),
                },
                "expected_epoch": None,
                "now_ms": int(time.time() * 1000),
            }
        )


def _fragment(graph, name: str) -> str:
    """The PROFILE clause of the REMOTE fragment a join over ``name`` ran."""
    profile = graph.query.uql(
        f"PROFILE MATCH (:PeDoc) |> FOREIGN SCAN '{name}' JOIN",
    )
    clauses = [stage["clause"] for stage in profile["stages"]]
    return next(clause for clause in clauses if clause.startswith("REMOTE "))


def _register_table(graph, pg_source, query: str | None = None) -> tuple[str, str]:
    """A fresh table of ``ROWS`` rows as a named SQL source; ``(table, name)``."""
    table = f"pe_items_{uuid.uuid4().hex[:8]}"
    pg_source.execute(f"CREATE TABLE {table} (id int primary key, name text)")
    pg_source.execute(
        f"INSERT INTO {table} SELECT g, 'item-' || g FROM generate_series(1, {ROWS}) g"
    )
    name = f"pe-{table}"
    graph.query.register_foreign_source(
        name,
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": query or f"SELECT id::text AS id FROM {table}",
                "id_field": "id",
            }
        },
    )
    return table, name


def _observe_shape(graph, name: str, joins: int = 7, reads: int = 12) -> None:
    """Record one source's shape: joins first (bind join), then whole reads."""
    join = [{"Scan": {"label": "PeDoc"}}, _scan(name, True)]
    for _ in range(joins):
        assert len(graph.query.unified(join)) == LOCAL_IDS
    for _ in range(reads):
        assert len(graph.query.unified([_scan(name, False)])) > 0


def _kept_digest(graph, name: str, seconds: int = 150) -> str:
    """Wait for the downtime tick to keep a plan for ``name``; its digest, or ``""``."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        time.sleep(2)
        clause = _fragment(graph, name)
        if "kept_plan=" in clause:
            return clause.split("kept_plan=", 1)[1].split()[0]
    return ""


def _explain(graph, name: str) -> dict[str, Any]:
    explained = graph.query.uql(f"EXPLAIN MATCH (:PeDoc) |> FOREIGN SCAN '{name}' JOIN")
    assert explained["kind"] == "explain"
    return explained


def _explain_kept(graph, name: str) -> list[dict[str, Any]]:
    return _explain(graph, name).get("kept_plans", [])


def _explain_baseline(graph, name: str) -> str | None:
    """The baseline plan digest ``EXPLAIN`` names while no plan is kept."""
    return _explain(graph, name).get("baseline_digest")


def _seed_locals(graph) -> None:
    for i in range(1, LOCAL_IDS + 1):
        graph.nodes.add(str(i), {"type": "PeDoc"})


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R091.3")
def test_served_query_records_and_later_query_reads_kept_plan(clean_graph, pg_source):
    _, name = _register_table(clean_graph, pg_source)
    _seed_locals(clean_graph)
    _provision_downtime(clean_graph)

    # Joins first, while the source is unobserved: the cost model picks the bind join.
    # Whole reads after: the baseline plan is the one a shape runs most often.
    _observe_shape(clean_graph, name)

    # The shape was recorded, so its fragment shows no kept plan yet.
    assert "kept_plan=" not in _fragment(clean_graph, name)

    # The downtime tick runs the plan-evolution job, which keeps the bind join (fewer
    # rows fetched, equal results); the next join over the same source reads the kept
    # plan.
    kept = _kept_digest(clean_graph, name)
    assert len(kept) == 64 and all(c in "0123456789abcdef" for c in kept), kept


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R091.5")
def test_explain_names_kept_plan_and_receipt(clean_graph, pg_source):
    table, name = _register_table(clean_graph, pg_source)
    _seed_locals(clean_graph)
    _provision_downtime(clean_graph)
    _observe_shape(clean_graph, name)
    assert _explain_kept(clean_graph, name) == []

    kept = _kept_digest(clean_graph, name)
    assert kept, "the plan_evolution job kept no plan"
    report = _explain_kept(clean_graph, name)[0]
    assert report["plan_digest"] == kept
    assert report["receipt_job_id"]
    assert report["baseline_digest"] and report["baseline_digest"] != kept
    assert report["replays"] >= int(os.environ["MEDULLA_FEDERATION_MIN_REPLAYS"])
    for field in ("p50_before_us", "p50_after_us", "bytes_before", "bytes_after"):
        assert isinstance(report[field], int) and report[field] >= 0

    # The evidence is kept with the plan under the owner: another tenant lists none,
    # even over a source registered under the same name.
    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=tenant_request_context("tenant:plan-evolution-other"),
    )
    try:
        other.query.register_foreign_source(
            name,
            {
                "Sql": {
                    "dsn": pg_source.dsn,
                    "query": f"SELECT id::text AS id FROM {table}",
                    "id_field": "id",
                }
            },
        )
        assert _explain_kept(other, name) == []
    finally:
        other.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R091.6")
def test_kept_plan_measurably_faster_and_mismatch_reverts(clean_graph, pg_source):
    _provision_downtime(clean_graph)
    _seed_locals(clean_graph)
    # One shape: 50 runs (the bind joins and the whole reads together).
    _, name = _register_table(clean_graph, pg_source)
    _observe_shape(clean_graph, name, joins=20, reads=30)
    # A second shape whose source answers a different set of ids on every read, so the
    # candidate's replays never equal the baseline's.
    table = f"pe_items_{uuid.uuid4().hex[:8]}"
    pg_source.execute(f"CREATE TABLE {table} (id int primary key, name text)")
    pg_source.execute(
        f"INSERT INTO {table} SELECT g, 'item-' || g FROM generate_series(1, {ROWS}) g"
    )
    unequal = f"pe-unequal-{table}"
    clean_graph.query.register_foreign_source(
        unequal,
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": (
                    f"SELECT id::text AS id FROM {table} ORDER BY random() LIMIT 10"
                ),
                "id_field": "id",
            }
        },
    )
    _observe_shape(clean_graph, unequal, joins=20, reads=30)

    # The downtime tick keeps the measurably faster plan; a later query reads it.
    assert _kept_digest(clean_graph, name), "the plan_evolution job kept no plan"
    report = _explain_kept(clean_graph, name)[0]
    assert (
        report["p50_after_us"] * 10 <= report["p50_before_us"] * 9
        or report["bytes_after"] * 10 <= report["bytes_before"] * 9
    ), report

    # The unequal candidate reverted: no kept plan, the baseline stays in force.
    time.sleep(6)
    assert "kept_plan=" not in _fragment(clean_graph, unequal)
    assert _explain_kept(clean_graph, unequal) == []


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R091.4")
def test_evolution_refusals(clean_graph, pg_source):
    """A candidate whose replays differ from the baseline's is refused
    (``PLAN_RESULT_MISMATCH``); the active plan stays the baseline."""
    _provision_downtime(clean_graph)
    _seed_locals(clean_graph)
    table = f"pe_items_{uuid.uuid4().hex[:8]}"
    pg_source.execute(f"CREATE TABLE {table} (id int primary key, name text)")
    pg_source.execute(
        f"INSERT INTO {table} SELECT g, 'item-' || g FROM generate_series(1, {ROWS}) g"
    )
    unequal = f"pe-refused-{table}"
    clean_graph.query.register_foreign_source(
        unequal,
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": (
                    f"SELECT id::text AS id FROM {table} ORDER BY random() LIMIT 10"
                ),
                "id_field": "id",
            }
        },
    )
    _observe_shape(clean_graph, unequal, joins=20, reads=30)
    before = _explain_baseline(clean_graph, unequal)

    # Give the downtime tick time to evaluate the shape; it refuses the candidate.
    time.sleep(30)
    assert "kept_plan=" not in _fragment(clean_graph, unequal)
    assert _explain_kept(clean_graph, unequal) == []
    assert _explain_baseline(clean_graph, unequal) == before
