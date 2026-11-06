"""Child message admission over the authenticated native TCP wire."""

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

    yield from served.authenticated_plan_session(request.getfixturevalue("tmp_path"))


def _run(run_id: str, child_input: dict | None = None) -> dict:
    now = int(time.time() * 1000)
    request = _submit_request()
    request.update(
        run_id=run_id,
        work_item_id=None,
        idempotency_key=run_id,
        kind="agent_turn",
        child_input=child_input,
        # The child run and each message admission are in flight together.
        max_tenant_in_flight=64,
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


def _input(child: str, key: str, digest: str, revision: int, state="queued") -> dict:
    return {
        "child_run_id": child,
        "expected_revision": revision,
        "content_digest": digest,
        "idempotency_key": key,
        "state": state,
    }


# spec: MEDULLA-DURABLE-KERNEL-R865
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R865")
def test_child_input_record_reads_over_the_wire(queue_client):
    client = queue_client
    stem = uuid.uuid4().hex
    child = "kid-" + stem
    client.work_items.submit(_run(child))
    a, b = "a" * 64, "b" * 64

    first = client.work_items.submit(_run("m1-" + stem, _input(child, "k1", a, 0)))
    assert first["changed_work_item_ids"]
    # Same key and digest replay; a different digest conflicts.
    client.work_items.submit(_run("m2-" + stem, _input(child, "k1", a, 0)))
    with pytest.raises(EngineResponseError, match="IDEMPOTENCY_CONFLICT"):
        client.work_items.submit(_run("m3-" + stem, _input(child, "k1", b, 0)))
    # A stale expected revision refuses without writing the message run.
    with pytest.raises(EngineResponseError, match="REVISION_CONFLICT"):
        client.work_items.submit(_run("m4-" + stem, _input(child, "k2", a, 7)))
    assert client.work_items.get_run(tenant=TEST_TENANT, run_id="m4-" + stem) is None
