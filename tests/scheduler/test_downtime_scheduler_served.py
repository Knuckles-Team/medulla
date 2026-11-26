"""The downtime scheduler over a served engine (MEDULLA-DURABLE-KERNEL-R101.6).

The engine is built with the `test-support` feature, whose `test_downtime_steps` body
takes 100 ms steps for 20 steps. Idle work runs in priority order, foreground load
preempts it at a step boundary through the J13 reclaim, and the jobs resume from their
checkpoint afterwards.
"""

import time

import msgpack
import pytest
from test_native_no_auth_served import ok, rpc, server

pytestmark = [
    pytest.mark.no_engine,
    pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R101.6"),
]

INTENT = "test_downtime_steps"
IDLE_MAX_RPS = 5
PREEMPT_DEADLINE_MS = 250
CPU_BUDGET_MS = 10_000
KNOBS = (
    "downtime.idle_quiet_secs=10",
    "downtime.tick_secs=1",
    f"downtime.idle_max_rps={IDLE_MAX_RPS}",
    "downtime.window_cpu_permille=1000",
)
# (node id, priority, precondition that holds)
TYPES = (
    ("dw:type-high", 30, True),
    ("dw:type-mid", 20, True),
    ("dw:type-low", 10, False),
)


def decoded(value):
    """A `Raw` payload arrives as MessagePack bytes; a typed one is already a value."""
    return msgpack.unpackb(value, raw=False) if isinstance(value, bytes) else value


def add_node(sock, node_id, properties):
    ok(
        rpc(
            sock,
            "AddNode",
            {
                "node_id": node_id,
                "properties_msgpack": msgpack.packb(properties),
            },
        )
    )


def add_edge(sock, source, target, relationship):
    ok(
        rpc(
            sock,
            "AddEdge",
            {
                "source_id": source,
                "target_id": target,
                "properties_msgpack": msgpack.packb({"relationship": relationship}),
            },
        )
    )


def provision(sock):
    """The two capacity cells and the three job types of the proof."""
    now = int(time.time() * 1000)
    for cell_id, resource_class, capacity in (
        ("downtime:cpu", "cpu", 1_000_000),
        ("downtime:memory", "memory", 1 << 32),
    ):
        ok(
            rpc(
                sock,
                "UpdateCapacityCell",
                {
                    "request": {
                        "schema_version": "1",
                        "cell": {
                            "cell_id": cell_id,
                            "resource_class": resource_class,
                            "capacity": capacity,
                            "reserved_floor": 0,
                            "epoch": 1,
                            "policy_digest": "downtime-scheduler-served",
                            "updated_at_ms": now,
                        },
                        "expected_epoch": None,
                        "now_ms": now,
                    }
                },
            )
        )
    for node_id, priority, holds in TYPES:
        add_node(
            sock,
            node_id,
            {
                "type": "JobType",
                "intent": INTENT,
                "priority": priority,
                "windowKind": "idle",
            },
        )
        budget = f"{node_id}-budget"
        add_node(
            sock,
            budget,
            {"type": "Budget", "cpuMs": CPU_BUDGET_MS, "memoryBytes": 1_048_576},
        )
        add_edge(sock, node_id, budget, "hasBudget")
        if not holds:
            check = f"{node_id}-precondition"
            add_node(
                sock,
                check,
                {
                    "type": "Precondition",
                    "query": "ASK { ?s a <https://example.invalid/Never> }",
                },
            )
            add_edge(sock, node_id, check, "hasPrecondition")


def selections(sock):
    """The recorded selections, newest first."""
    page = decoded(ok(rpc(sock, "DowntimeSelections", {"limit": 10})))
    return page["selections"]


def wait_until(check, *, seconds, interval=0.05):
    """Poll `check` until it returns a truthy value or `seconds` pass."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        found = check()
        if found:
            return found
        time.sleep(interval)
    pytest.fail("the engine did not reach the awaited state in time")


def chosen_of(sock):
    """The chosen entries of the newest selection that chose anything."""
    for selection in selections(sock):
        if selection["chosen"]:
            return selection
    return None


def observations(sock, selection_id):
    for selection in selections(sock):
        if selection["selection_id"] == selection_id:
            return [entry["job"] for entry in selection["chosen"]]
    return []


# spec: MEDULLA-DURABLE-KERNEL-R101.6
def test_idle_window_runs_priority_jobs_and_preempts(tmp_path):
    with server(
        tmp_path,
        features=("test-support",),
        extra_args=[arg for knob in KNOBS for arg in ("--knob", knob)],
    ) as sock:
        provision(sock)

        # After the quiet window the tick chooses the two eligible types in priority
        # order and skips the third by its precondition.
        selection = wait_until(lambda: chosen_of(sock), seconds=40, interval=0.5)
        assert [
            entry["iri"].rsplit("#", 1)[-1].rsplit(":", 1)[-1]
            for entry in selection["chosen"]
        ] == [
            "type-high",
            "type-mid",
        ]
        assert {entry["intent"] for entry in selection["chosen"]} == {INTENT}
        assert [
            (entry["iri"].rsplit(":", 1)[-1], entry["reason"])
            for entry in selection["skipped"]
        ] == [("type-low", "precondition_false")]

        # Each chosen job is running under a worker lease.
        def all_running():
            jobs = observations(sock, selection["selection_id"])
            return (
                jobs
                if jobs and all(j and j["state"] == "running" for j in jobs)
                else None
            )

        running = wait_until(all_running, seconds=20, interval=0.5)
        assert all(job["has_lease"] for job in running)
        attempts = [job["attempts_made"] for job in running]

        # More than `idle_max_rps` requests in one second preempt the work: within the
        # deadline every job left `Running` for `Submitted` with its checkpoint and the
        # same attempt count.
        for _ in range(IDLE_MAX_RPS + 3):
            ok(rpc(sock, "Ping"))
        started = time.monotonic()

        def all_yielded():
            jobs = observations(sock, selection["selection_id"])
            return (
                jobs
                if jobs and all(j and j["state"] == "submitted" for j in jobs)
                else None
            )

        yielded = wait_until(
            all_yielded, seconds=PREEMPT_DEADLINE_MS / 1000, interval=0.025
        )
        assert time.monotonic() - started <= PREEMPT_DEADLINE_MS / 1000 + 0.5
        for job, before in zip(yielded, attempts, strict=True):
            assert job["has_lease"] is False
            assert job["has_checkpoint"] is True
            assert job["checkpoint_stage"] == "downtime_step"
            assert job["attempts_made"] == before

        # After the burst, polling below the idle ceiling, the jobs resume from their
        # checkpoint and finish inside their budget.
        def all_done():
            jobs = observations(sock, selection["selection_id"])
            return (
                jobs
                if jobs and all(j and j["state"] == "succeeded" for j in jobs)
                else None
            )

        done = wait_until(all_done, seconds=60, interval=0.5)
        for job in done:
            assert job["cpu_ms"] is not None and job["cpu_ms"] <= CPU_BUDGET_MS
