"""MEDULLA-DURABLE-KERNEL-R028.7: ``ClaimWorkItem`` answers the run's identity."""

import time
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT
from test_child_input_served import _run

pytestmark = [pytest.mark.no_engine]

DIGEST = "sha256:" + "c" * 64


@pytest.fixture
def queue_client(request):
    from decide import test_run_work_item_served as served

    yield from served.authenticated_plan_session(request.getfixturevalue("tmp_path"))


def _claim(client, work_item_id: str) -> dict:
    return client.work_items.claim(
        {
            "schema_version": "1",
            "tenant_ref": TEST_TENANT,
            "work_item_id": work_item_id,
            "queue_ref": None,
            "resource_class": None,
            "fairness_group": None,
            "worker_ref": "worker-1",
            "now_ms": int(time.time() * 1000),
            "lease_ms": 60_000,
            "max_tenant_in_flight": 64,
        }
    )


# spec: MEDULLA-DURABLE-KERNEL-R028.7
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R028.7")
def test_a_claim_answers_the_identity_the_item_was_submitted_with(queue_client):
    stem = uuid.uuid4().hex[:10]
    request = _run("run-" + stem)
    request["metadata"] = {
        "capability_digest": DIGEST,
        "delegation_id": "delegation-" + stem,
        "agent_id": "selected-agent",
    }
    submitted = queue_client.work_items.submit(request)
    claim = _claim(queue_client, submitted["work_item_id"])
    assert claim["claimed"] is True
    assert claim["capability_digest"] == DIGEST
    assert claim["delegation_id"] == "delegation-" + stem
    assert claim["selected_agent_id"] == "selected-agent"
    assert claim["delegator_id"] == TEST_AGENT_ID
    assert claim["catalog_digest"] == request["catalog_digest"]
    assert claim["model_digest"] == request["model_digest"]
    assert claim["policy_digest"] == request["policy_digest"]
    assert claim["lease_holder_ref"] == "worker-1"
