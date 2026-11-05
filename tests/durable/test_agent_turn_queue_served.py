"""Agent-turn WorkItem queue refusals over the authenticated native TCP wire."""

import copy
import time
import uuid

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_AUDIENCE,
    TEST_POLICY_VERSION,
    TEST_TENANT,
)
from test_work_item_retry_identity import _submit_request

from medulla.client import EngineResponseError

pytestmark = [pytest.mark.no_engine]


@pytest.fixture
def queue_client(request):
    from decide import test_run_work_item_served as served

    yield from served.authenticated_plan_client.__pytest_wrapped__.obj(
        request.getfixturevalue("tmp_path")
    )


def _turn(run_id: str, *, max_in_flight: int = 4096, conversation=None) -> dict:
    now = int(time.time() * 1000)
    request = _submit_request()
    request.update(
        run_id=run_id,
        work_item_id=None,
        idempotency_key=run_id,
        kind="agent_turn",
        max_tenant_in_flight=max_in_flight,
        conversation=conversation,
    )
    request["context"].update(
        request_id=run_id,
        trace_id=run_id,
        subject_id=TEST_AGENT_ID,
        tenant_id=TEST_TENANT,
        agent_id=TEST_AGENT_ID,
        audience=TEST_AUDIENCE,
        policy_version=TEST_POLICY_VERSION,
        graph="__commons__",
        scopes=["*"],
        issued_at_ms=now,
        expires_at_ms=now + 60_000,
    )
    return request


def _absent(client, run_id: str) -> bool:
    return client.work_items.get_run(tenant=TEST_TENANT, run_id=run_id) is None


# spec: MEDULLA-DURABLE-KERNEL-R750.752
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R750.752")
def test_full_queue_stale_head_and_bad_capability_refuse(queue_client):
    client = queue_client
    stem = uuid.uuid4().hex
    now = int(time.time() * 1000)
    first = client.work_items.submit(_turn("full-a-" + stem, max_in_flight=1))

    # In-flight count meets max_tenant_in_flight: typed refusal, nothing written.
    with pytest.raises(EngineResponseError, match="WORK_QUEUE_FULL"):
        client.work_items.submit(_turn("full-b-" + stem, max_in_flight=1))
    assert _absent(client, "full-b-" + stem)

    # A second turn on a context whose head moved refuses with the conflict.
    head = {
        "schema_version": "1",
        "context_id": "ctx-" + stem,
        "expected_revision": 0,
        "predecessor_run_id": None,
    }
    # Cancel the full-queue item first so the context turns are admitted.
    client.work_items.cancel(
        tenant=TEST_TENANT,
        work_item_id=first["work_item_id"],
        idempotency_key=stem + ":cancel-1",
        now_ms=now,
    )
    client.work_items.submit(_turn("ctx-a-" + stem, conversation=copy.deepcopy(head)))
    with pytest.raises(EngineResponseError, match="CONVERSATION_CONFLICT"):
        client.work_items.submit(_turn("ctx-b-" + stem, conversation=head))
    assert _absent(client, "ctx-b-" + stem)

    # An unverifiable claim capability refuses as ACCESS_DENIED.
    denied = client.work_items.verify_capability(
        {
            "schema_version": "1",
            "work_item_id": first["work_item_id"],
            "capability": b"forged",
        }
    )
    assert denied["valid"] is False
    assert denied["decision"] in {"unauthorized", "malformed"}

    # Cancel of a terminal item is a Noop and changes nothing.
    terminal = client.work_items.get(
        tenant=TEST_TENANT, work_item_id=first["work_item_id"]
    )
    again = client.work_items.cancel(
        tenant=TEST_TENANT,
        work_item_id=first["work_item_id"],
        idempotency_key=stem + ":cancel-2",
        now_ms=now,
    )
    assert again["status"] == "noop"
    assert (
        client.work_items.get(tenant=TEST_TENANT, work_item_id=first["work_item_id"])
        == terminal
    )

    # Cross-tenant: a foreign tenant sees no item and the refusal touched none.
    assert (
        client.work_items.get(
            tenant="tenant-foreign", work_item_id=first["work_item_id"]
        )
        is None
    )
