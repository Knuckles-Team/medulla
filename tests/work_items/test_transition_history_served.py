"""A WorkItem's transition ledger over the authenticated native wire."""

import time
import uuid

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_AUDIENCE,
    TEST_POLICY_VERSION,
    TEST_TENANT,
)
from durable.test_uncertain_effect_served import effect_client  # noqa: F401
from test_work_item_retry_identity import _submit_request

pytestmark = [pytest.mark.no_engine]


def _submit(client, stem: str) -> str:
    now = int(time.time() * 1000)
    request = _submit_request()
    item = "history-" + stem
    request.update(work_item_id=item, idempotency_key=item)
    request["context"].update(
        request_id=item,
        trace_id=item,
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
    return client.work_items.submit(request)["work_item_id"]


def _pages(client, item: str, limit: int) -> list[dict]:
    pages: list[dict] = []
    cursor = None
    while True:
        page = client.work_items.list_transitions(
            tenant=TEST_TENANT, work_item_id=item, cursor=cursor, limit=limit
        )
        assert page is not None
        pages.append(page)
        cursor = page["next_cursor"]
        if cursor is None:
            return pages


# spec: MEDULLA-DURABLE-KERNEL-R028.2.1
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R028.2.1")
def test_history_matches_ledger_in_commit_order(effect_client):  # noqa: F811
    client = effect_client
    stem = uuid.uuid4().hex
    item = _submit(client, stem)
    claim = client.work_items.claim(
        {
            "schema_version": "1",
            "tenant_ref": TEST_TENANT,
            "work_item_id": item,
            "queue_ref": None,
            "resource_class": None,
            "fairness_group": None,
            "worker_ref": "worker-" + stem,
            "now_ms": int(time.time() * 1000),
            "lease_ms": 30_000,
            "max_tenant_in_flight": 4096,
        },
        idempotency_key=f"{stem}:claim",
    )
    assert claim["claimed"] is True
    fence = {
        "tenant": TEST_TENANT,
        "work_item_id": item,
        "worker_id": claim["lease_holder_ref"],
        "lease_epoch": claim["lease_epoch"],
        "fencing_token": claim["fencing_token"],
    }
    # The first renewal starts the work; the second keeps the status, appends none.
    for key in ("first", "second"):
        client.work_items.renew(
            **fence,
            now_ms=int(time.time() * 1000),
            lease_ms=30_000,
            idempotency_key=f"{stem}:renew-{key}",
        )
    client.work_items.commit_result(
        **fence,
        idempotency_key=f"{stem}:commit",
        outcome="succeeded",
        now_ms=int(time.time() * 1000),
        result_ref="cas:result:history",
    )

    whole = _pages(client, item, 100)
    assert len(whole) == 1 and whole[0]["next_cursor"] is None
    entries = whole[0]["entries"]
    assert [entry["state"] for entry in entries] == [
        "ready",
        "leased",
        "running",
        "succeeded",
    ]
    revisions = [entry["revision"] for entry in entries]
    assert revisions == sorted(revisions) and len(set(revisions)) == 4

    paged = _pages(client, item, 2)
    assert [len(page["entries"]) for page in paged] == [2, 2]
    assert [entry for page in paged for entry in page["entries"]] == entries

    # An absent WorkItem answers null, exactly as GetWorkItem does.
    assert (
        client.work_items.list_transitions(
            tenant=TEST_TENANT, work_item_id="history-absent-" + stem
        )
        is None
    )
    # A limit outside [1, 200] is refused before the store is read.
    with pytest.raises(ValueError):
        client.work_items.list_transitions(
            tenant=TEST_TENANT, work_item_id=item, limit=201
        )
