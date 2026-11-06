"""Served run history: a terminal commit stores steps; HistorySearch finds them.

MEDULLA-UNIFIED-DATA-PLANE-R750.753 / R750.754.
"""

import asyncio
import hashlib
import time
import uuid

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import EngineResponseError

pytestmark = [pytest.mark.no_engine]


@pytest.fixture
def history_client(request):
    from decide import test_run_work_item_served as served

    yield from served.authenticated_plan_client.__pytest_wrapped__.obj(
        request.getfixturevalue("tmp_path")
    )


def _redact(raw: str) -> str:
    return "redacted:" + hashlib.sha256(raw.encode()).hexdigest()[:16]


def _search(client, scope: str, query: str, *, scopes: list[str] | None = None):
    params = {"scope": scope, "query": query, "k": 10}

    async def call():
        if scopes is None:
            return await client._client._send("HistorySearch", params)
        with client._client.use_verified_context(request_context(scopes=scopes)):
            return await client._client._send("HistorySearch", params)

    return asyncio.run_coroutine_threadsafe(call(), client._loop).result()


def _commit_run(client, scope: str, steps: list[str]) -> str:
    from durable.test_agent_turn_queue_served import _turn

    stem = uuid.uuid4().hex
    run_id = "history-" + stem
    now = int(time.time() * 1000)
    item = client.work_items.submit(_turn(run_id))["work_item_id"]
    claim = client.work_items.claim(
        {
            "schema_version": "1",
            "tenant_ref": TEST_TENANT,
            "work_item_id": item,
            "queue_ref": None,
            "resource_class": None,
            "fairness_group": None,
            "worker_ref": "worker-" + stem,
            "now_ms": now,
            "lease_ms": 30_000,
            "max_tenant_in_flight": 4096,
        },
        idempotency_key=stem + ":claim",
    )
    speaker = _redact("alice@example.org")
    client.work_items.commit_result(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=claim["lease_holder_ref"],
        lease_epoch=claim["lease_epoch"],
        fencing_token=claim["fencing_token"],
        idempotency_key=stem + ":commit",
        outcome="succeeded",
        now_ms=now + 1,
        run_completion={
            "schema_version": 1,
            "run_id": run_id,
            "history": {
                "scope": scope,
                "actor": speaker,
                "trace_node": "trace:" + run_id,
                "tool_calls": [
                    {
                        "call_id": "c1",
                        "tool": "graph_query",
                        "trace_node": "trace:" + run_id + "/c1",
                        "args_digest": "sha256:00",
                    }
                ],
                "steps": [
                    {"index": i, "speaker": speaker, "text": text}
                    for i, text in enumerate(steps)
                ],
            },
        },
    )
    return run_id


# spec: MEDULLA-UNIFIED-DATA-PLANE-R750.754
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R750.754")
def test_commit_writes_audit_and_search_finds_step(history_client):
    client = history_client
    run_id = _commit_run(
        client, "project:alpha", ["plan the quarterly release", "ship the wheel"]
    )
    hits = _search(client, "project:alpha", "quarterly")
    assert [(h["run_id"], h["index"]) for h in hits] == [(run_id, 0)]


# spec: MEDULLA-UNIFIED-DATA-PLANE-R750.753
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R750.753")
def test_search_is_scope_filtered_before_ranking(history_client):
    client = history_client
    alpha = _commit_run(client, "project:alpha", ["deploy the gateway"])
    _commit_run(client, "project:beta", ["deploy the gateway twice"])
    hits = _search(client, "project:alpha", "deploy gateway")
    assert {h["run_id"] for h in hits} == {alpha}
    with pytest.raises(EngineResponseError) as refused:
        _search(
            client, "project:beta", "deploy", scopes=["history:read", "project:alpha"]
        )
    assert "HISTORY_SCOPE_FORBIDDEN" in str(refused.value)
