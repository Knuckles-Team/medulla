"""MEDULLA-FEDERATED-QUERY-R091.3 served acceptance: a kept plan is read by a later query.

A served query over a registered foreign source records one observation of its shape under
the verified owner. The downtime tick submits the ``plan_evolution`` job, the engine runs it
in process (replaying the baseline and the candidate plan against the source), keeps the
candidate that fetches fewer rows with the same results, and completes the job. A later join
over the same source then reads the kept plan: its PROFILE fragment names the kept digest.
"""

from __future__ import annotations

import os
import time
import uuid

import pytest

# The knobs resolve from the environment of the server process, which the session fixture
# starts from this process's environment: name them at collection time.
os.environ.setdefault("MEDULLA_DOWNTIME_TICK_SECS", "1")
os.environ.setdefault("MEDULLA_DOWNTIME_IDLE_QUIET_SECS", "10")
os.environ.setdefault("MEDULLA_FEDERATION_MIN_REPLAYS", "5")

DW = "https://knuckles-team.github.io/ontology/downtime-work#"
ROWS = 50
LOCAL_IDS = 8


def _scan(name: str, join: bool) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": join}}


def _provision_downtime(graph) -> None:
    """One idle ``plan_evolution`` job type and the two capacity cells its lease draws on."""
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


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R091.3")
def test_served_query_records_and_later_query_reads_kept_plan(clean_graph, pg_source):
    table = f"pe_items_{uuid.uuid4().hex[:8]}"
    pg_source.execute(f"CREATE TABLE {table} (id int primary key, name text)")
    pg_source.execute(
        f"INSERT INTO {table} SELECT g, 'item-' || g FROM generate_series(1, {ROWS}) g"
    )
    name = f"pe-{table}"
    clean_graph.query.register_foreign_source(
        name,
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": f"SELECT id::text AS id FROM {table}",
                "id_field": "id",
            }
        },
    )
    for i in range(1, LOCAL_IDS + 1):
        clean_graph.nodes.add(str(i), {"type": "PeDoc"})
    _provision_downtime(clean_graph)

    # Joins first, while the source is unobserved: the cost model picks the bind join. Whole
    # reads after: the baseline plan is the one a shape runs most often.
    join = [{"Scan": {"label": "PeDoc"}}, _scan(name, True)]
    for _ in range(7):
        rows = clean_graph.query.unified(join)
        assert len(rows) == LOCAL_IDS
    for _ in range(12):
        assert len(clean_graph.query.unified([_scan(name, False)])) == ROWS

    # The shape was recorded, so its fragment shows no kept plan yet.
    assert "kept_plan=" not in _fragment(clean_graph, name)

    # The downtime tick runs the plan-evolution job, which keeps the bind join (fewer rows
    # fetched, equal results); the next join over the same source reads the kept plan.
    deadline = time.time() + 150
    kept = ""
    while time.time() < deadline:
        time.sleep(2)
        clause = _fragment(clean_graph, name)
        if "kept_plan=" in clause:
            kept = clause.split("kept_plan=", 1)[1].split()[0]
            break
    assert len(kept) == 64 and all(c in "0123456789abcdef" for c in kept), kept
