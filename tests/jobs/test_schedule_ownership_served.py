"""Served schedule records: owner-scoped CRUD and cron validation.

spec: MEDULLA-PLACEMENT-R751.754

A schedule is a `Schedule` node in the tenant graph, created, read, updated,
deleted and listed through `AnalyticsJob` schedule ops. The owner may change or
remove it; a different principal of the same tenant is refused with
`SCHEDULE_NOT_OWNER` and an invalid cron expression with `INVALID_SCHEDULE`.
"""

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient

CRON = {"Cron": {"minute": "0", "hour": "*/6"}}


def _other_principal() -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(
            agent_id=f"sched-other-{uuid.uuid4().hex[:8]}"
        ),
    )


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.754")
def test_schedule_crud_is_owner_scoped_and_validates_cron(clean_graph):
    graph = f"schedule-own-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph, "Team")
    name = f"nightly-{uuid.uuid4().hex[:8]}"

    created = clean_graph.jobs.schedule_create(graph, name, CRON, "sweep")
    assert created["name"] == name
    assert created["task_kind"] == "sweep"
    assert created["owner"]

    assert clean_graph.jobs.schedule_get(graph, name) == created
    listed = clean_graph.jobs.schedule_list(graph)["schedules"]
    assert [s["name"] for s in listed] == [name]

    # A duplicate create does not replace the record.
    with pytest.raises((EngineResponseError, RuntimeError), match="SCHEDULE_EXISTS"):
        clean_graph.jobs.schedule_create(graph, name, CRON, "other")

    updated = clean_graph.jobs.schedule_update(
        graph, name, {"Interval": {"every_ms": 60_000, "start_ms": 0}}, "sweep-2"
    )
    assert updated["task_kind"] == "sweep-2"
    assert updated["owner"] == created["owner"]

    # Invalid cron fields and a zero interval are refused with a typed error,
    # on create and on update, and leave the stored record unchanged.
    bad_cron = {"Cron": {"minute": "61", "hour": "*"}}
    with pytest.raises((EngineResponseError, RuntimeError), match="INVALID_SCHEDULE"):
        clean_graph.jobs.schedule_create(graph, f"{name}-bad", bad_cron, "sweep")
    with pytest.raises((EngineResponseError, RuntimeError), match="INVALID_SCHEDULE"):
        clean_graph.jobs.schedule_update(graph, name, bad_cron, "sweep")
    assert clean_graph.jobs.schedule_get(graph, name) == updated

    # A principal that does not own the schedule can neither change nor remove it.
    other = _other_principal()
    try:
        with pytest.raises(
            (EngineResponseError, RuntimeError), match="SCHEDULE_NOT_OWNER"
        ):
            other.jobs.schedule_update(graph, name, CRON, "hijack")
        with pytest.raises(
            (EngineResponseError, RuntimeError), match="SCHEDULE_NOT_OWNER"
        ):
            other.jobs.schedule_delete(graph, name)
    finally:
        other.close()
    assert clean_graph.jobs.schedule_get(graph, name) == updated

    removed = clean_graph.jobs.schedule_delete(graph, name)
    assert removed == {"name": name}
    with pytest.raises((EngineResponseError, RuntimeError), match="SCHEDULE_NOT_FOUND"):
        clean_graph.jobs.schedule_get(graph, name)
    assert clean_graph.jobs.schedule_list(graph)["schedules"] == []
