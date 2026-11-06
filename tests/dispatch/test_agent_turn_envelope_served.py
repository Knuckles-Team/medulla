"""The signed agent-turn envelope over the authenticated native TCP wire."""

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


def _turn(run_id: str, envelope: dict) -> dict:
    now = int(time.time() * 1000)
    request = _submit_request()
    request.update(
        run_id=run_id,
        work_item_id=None,
        idempotency_key=run_id,
        kind="agent_turn",
        metadata={"dispatch_carrier": {"envelope": envelope, "signature": "alice:00"}},
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


def _envelope(run_id: str, **fields) -> dict:
    base = {
        "run_id": run_id,
        "declares_harness_decision": True,
        "allowed_tool_subset": ["read", "search"],
        "decision_record_ref": "decision:1",
        "harness_config_digest": "a" * 64,
    }
    return base | fields


# spec: MEDULLA-DURABLE-KERNEL-R750.750.1
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R750.750.1")
def test_signed_envelope_carries_subset_and_decision_ref(queue_client):
    """Verifies MEDULLA-DURABLE-KERNEL-R750.750.1: signed envelope fields."""
    client = queue_client
    stem = uuid.uuid4().hex
    ok = "env-ok-" + stem
    client.work_items.submit(_turn(ok, _envelope(ok)))
    stored = client.work_items.get_run(tenant=TEST_TENANT, run_id=ok)
    assert stored is not None

    for field in (
        "allowed_tool_subset",
        "decision_record_ref",
        "harness_config_digest",
    ):
        run = f"env-{field}-{stem}"
        with pytest.raises(EngineResponseError, match="ENVELOPE_FIELD_MISSING"):
            client.work_items.submit(_turn(run, _envelope(run, **{field: None})))
        assert client.work_items.get_run(tenant=TEST_TENANT, run_id=run) is None

    over = "env-over-" + stem
    with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
        client.work_items.submit(
            _turn(
                over, _envelope(over, allowed_tool_subset=[f"t{i}" for i in range(65)])
            )
        )
