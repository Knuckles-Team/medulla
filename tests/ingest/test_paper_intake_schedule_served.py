"""Served paper intake schedule: MEDULLA-REPO-INGEST-R006.6.

A schedule of task kind `paper_intake` fires one task on the ingest queue per boundary.
The task carries `run_id = sha256(schedule_id + boundary)`, the `cycle_boundary_ms` and
the platform goal ids, and nothing else is written.
"""

import hashlib
import time
import uuid

import pytest

GRAPH = "__commons__"
EVERY_MS = 2_000
WORKER = "paper-intake-test-worker"


def _wait_for(predicate, timeout_s: float):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        found = predicate()
        if found:
            return found
        time.sleep(0.5)
    return predicate()


@pytest.mark.spec("MEDULLA-REPO-INGEST-R006.6")
def test_firing_enqueues_one_task_per_boundary(clean_graph):
    name = f"paper-intake-{uuid.uuid4().hex[:8]}"
    now_ms = int(time.time() * 1000)
    created = clean_graph.jobs.schedule_create(
        GRAPH,
        name,
        {"interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "paper_intake",
    )
    schedule_id = f"schedule:{created['tenant']}:{name}"
    try:
        fired = _wait_for(
            lambda: [
                tick
                for tick in clean_graph.jobs.schedule_ticks(GRAPH, name)["ticks"]
                if tick["outcome"] == "fired"
            ],
            30,
        )
        assert fired, "a due boundary fires"
        boundary = fired[0]["window_start_ms"]
        run_id = hashlib.sha256(f"{schedule_id}{boundary}".encode()).hexdigest()
        assert fired[0]["work_item_id"] == run_id, "the fired tick links the queue row"

        clean_graph.task_queue.register_worker(
            worker_id=WORKER,
            host_iri="host:alpha",
            cores=4,
            memory_bytes=4 * 1024**3,
            lanes_supported=["general"],
        )
        grant = clean_graph.task_queue.admit_worker_task(
            worker_id=WORKER, requested_max=1
        )
        grant_id = getattr(grant, "grant_id", "grant-1")
        seen = []
        for _ in range(64):
            claimed = clean_graph.task_queue.claim(
                worker_id=WORKER, grant_id=grant_id, lease_ms=60_000
            )
            if claimed is None:
                break
            seen.append(claimed)
            clean_graph.task_queue.ack(
                task_id=claimed.task_id, worker_id=WORKER, outcome="Succeeded"
            )
        ours = [task for task in seen if task.task_id == run_id]
        assert len(ours) == 1, "exactly one queue row per boundary"
        assert ours[0].kind == "paper_intake"
        assert f'"cycle_boundary_ms":{boundary}' in ours[0].payload_ref
        assert f'"run_id":"{run_id}"' in ours[0].payload_ref
        assert '"goal_ids":[' in ours[0].payload_ref
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, name)
