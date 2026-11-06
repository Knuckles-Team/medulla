"""The uncertain-effect protocol over the authenticated native TCP wire."""

import hashlib
import socket
import time
import urllib.request
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
def effect_client(request):
    from decide import test_run_work_item_served as served

    yield from served.authenticated_plan_session(request.getfixturevalue("tmp_path"))


@pytest.fixture
def metered_client(request, monkeypatch):
    """The effect client of a server that also serves its metrics endpoint."""
    from decide import test_run_work_item_served as served

    with socket.socket(socket.AF_INET) as probe:
        probe.bind(("127.0.0.1", 0))
        address = "{}:{}".format(*probe.getsockname())
    served_env = served.strict_server_env
    monkeypatch.setattr(
        served,
        "strict_server_env",
        lambda *a, **k: served_env(*a, **k) | {"GRAPH_SERVICE_METRICS_ADDR": address},
    )
    client_iter = served.authenticated_plan_session(request.getfixturevalue("tmp_path"))
    client = next(client_iter)
    client.metrics_url = "http://" + address + "/metrics"
    yield client
    client_iter.close()


def _scrape(client) -> dict[str, float]:
    """Every sample line of the metrics endpoint, keyed by series."""
    with urllib.request.urlopen(client.metrics_url, timeout=10) as response:
        text = response.read().decode()
    return {
        line.rsplit(" ", 1)[0]: float(line.rsplit(" ", 1)[1])
        for line in text.splitlines()
        if line and not line.startswith("#")
    }


def _audit_lines(client) -> list[str]:
    """The event lines of the audit chain, oldest first."""
    lines = []
    for seq in range(1, 200):
        try:
            lines.append(client.admin.audit_read_event(seq)["event_line"])
        except EngineResponseError:
            break
    return lines


def _item(client, stem: str) -> str:
    now = int(time.time() * 1000)
    request = _submit_request()
    run_id = "effect-" + stem
    request.update(run_id=run_id, work_item_id=None, idempotency_key=run_id)
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
    return client.work_items.submit(request)["work_item_id"]


def _claim(
    client, item: str, stem: str, lease_ms: int = 30_000, key: str = "claim"
) -> dict:
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
            "lease_ms": lease_ms,
            "max_tenant_in_flight": 4096,
        },
        idempotency_key=f"{stem}:{key}",
    )
    assert claim["claimed"] is True
    return claim


def _view(client, item: str) -> dict:
    return client.work_items.get(tenant=TEST_TENANT, work_item_id=item)


def _admitted(payload: bytes, session: str = "session-a") -> dict:
    return {
        "request_ref": "sha256:" + hashlib.sha256(payload).hexdigest(),
        "tenant": TEST_TENANT,
        "session_ref": session,
        "payload_digest": "blob:" + hashlib.sha256(payload).hexdigest(),
        "bytes": list(payload),
    }


def _receipt(item: str, request_ref: str, principal: str, outcome="applied") -> dict:
    return {
        "job_id": item,
        "request_ref": request_ref,
        "principal_ref": principal,
        "outcome": outcome,
        "receipt_ref": "sha256:" + "b" * 64,
        "committed_at": 7,
    }


# spec: MEDULLA-DURABLE-KERNEL-R820.3
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R820.3")
def test_methods_reachable_through_wire_contract(effect_client):
    client = effect_client
    stem = uuid.uuid4().hex
    item = _item(client, stem)
    claim = _claim(client, item, stem)
    attempt = claim.get("attempt", 1)
    payload = b'{"call":"send"}'
    admitted = _admitted(payload)

    stored = client.work_items.submit_admitted_request(
        work_item_id=item,
        attempt=attempt,
        expected_revision=_view(client, item)["version"],
        request=admitted,
    )
    assert stored["replayed"] is False
    read = client.work_items.get_admitted_request(
        tenant=TEST_TENANT, session_ref="session-a", work_item_id=item, attempt=attempt
    )
    assert read["request_ref"] == admitted["request_ref"]
    assert read["payload_ref"] == admitted["payload_digest"]

    started = client.work_items.mark_effect_started(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=claim["lease_holder_ref"],
        lease_epoch=claim["lease_epoch"],
        fencing_token=claim["fencing_token"],
        expected_revision=_view(client, item)["version"],
    )
    assert _view(client, item)["effect_phase"] == "started"
    assert started["revision"] == _view(client, item)["version"]

    receipt = _receipt(item, admitted["request_ref"], claim["lease_holder_ref"])
    assert (
        client.work_items.get_effect_receipt(
            tenant=TEST_TENANT, work_item_id=item, attempt=attempt
        )
        is None
    )
    recorded = client.work_items.record_effect_receipt(
        tenant=TEST_TENANT,
        work_item_id=item,
        attempt=attempt,
        expected_revision=started["revision"],
        receipt=receipt,
    )
    assert recorded["receipt"] == receipt
    assert (
        client.work_items.get_effect_receipt(
            tenant=TEST_TENANT, work_item_id=item, attempt=attempt
        )
        == receipt
    )
    assert _view(client, item)["effect_phase"] == "resolved"

    # An identical replay returns the stored receipt and writes nothing.
    before = _view(client, item)
    replay = client.work_items.record_effect_receipt(
        tenant=TEST_TENANT,
        work_item_id=item,
        attempt=attempt,
        expected_revision=before["version"],
        receipt=receipt,
        idempotency_key=stem + ":replay",
    )
    assert replay["replayed"] is True
    assert replay["receipt"] == receipt
    assert _view(client, item) == before


# spec: MEDULLA-DURABLE-KERNEL-R820.4
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R820.4")
def test_refusals_leave_state_unchanged(effect_client):
    client = effect_client
    stem = uuid.uuid4().hex
    item = _item(client, stem)
    claim = _claim(client, item, stem)
    attempt = claim.get("attempt", 1)
    worker = claim["lease_holder_ref"]

    def unchanged(code: str, call) -> None:
        before = _view(client, item)
        with pytest.raises(EngineResponseError, match=code):
            call()
        assert _view(client, item) == before

    # A start mark without a live claim.
    unchanged(
        "WORK_ITEM_CLAIM_STALE",
        lambda: client.work_items.mark_effect_started(
            tenant=TEST_TENANT,
            work_item_id=item,
            worker_id=worker,
            lease_epoch=claim["lease_epoch"] + 7,
            fencing_token=claim["fencing_token"],
            expected_revision=_view(client, item)["version"],
            idempotency_key=stem + ":stale",
        ),
    )

    # Admitted-request digest and size refusals.
    payload = b'{"call":"send"}'
    tampered = _admitted(payload) | {"bytes": list(b'{"call":"drop"}')}
    unchanged(
        "ADMITTED_REQUEST_DIGEST_MISMATCH",
        lambda: client.work_items.submit_admitted_request(
            work_item_id=item,
            attempt=attempt,
            expected_revision=_view(client, item)["version"],
            request=tampered,
            idempotency_key=stem + ":tampered",
        ),
    )
    # ADMITTED_REQUEST_TOO_LARGE: a 1 MiB body exceeds the wire frame's item
    # bound before it reaches the handler, so the size refusal is bound by the
    # Rust test `admitted_request_refusals_are_registered_codes`.
    admitted = _admitted(payload)
    client.work_items.submit_admitted_request(
        work_item_id=item,
        attempt=attempt,
        expected_revision=_view(client, item)["version"],
        request=admitted,
    )
    # A foreign tenant or session reads not found.
    assert (
        client.work_items.get_admitted_request(
            tenant=TEST_TENANT,
            session_ref="session-b",
            work_item_id=item,
            attempt=attempt,
        )
        is None
    )
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
        client.work_items.get_admitted_request(
            tenant="tenant-foreign",
            session_ref="session-a",
            work_item_id=item,
            attempt=attempt,
        )

    started = client.work_items.mark_effect_started(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=worker,
        lease_epoch=claim["lease_epoch"],
        fencing_token=claim["fencing_token"],
        expected_revision=_view(client, item)["version"],
    )
    revision = started["revision"]

    # Receipt mismatches: job, request, principal and attempt.
    for name, receipt, at in (
        ("job", _receipt("other", admitted["request_ref"], worker), attempt),
        ("request", _receipt(item, "sha256:" + "c" * 64, worker), attempt),
        ("principal", _receipt(item, admitted["request_ref"], "intruder"), attempt),
        ("attempt", _receipt(item, admitted["request_ref"], worker), attempt + 1),
    ):
        unchanged(
            "EFFECT_RECEIPT_MISMATCH",
            lambda receipt=receipt, at=at, name=name: (
                client.work_items.record_effect_receipt(
                    tenant=TEST_TENANT,
                    work_item_id=item,
                    attempt=at,
                    expected_revision=revision,
                    receipt=receipt,
                    idempotency_key=f"{stem}:mismatch:{name}",
                )
            ),
        )

    receipt = _receipt(item, admitted["request_ref"], worker)
    client.work_items.record_effect_receipt(
        tenant=TEST_TENANT,
        work_item_id=item,
        attempt=attempt,
        expected_revision=revision,
        receipt=receipt,
    )
    # A second, different receipt under the same key.
    unchanged(
        "EFFECT_RECEIPT_CONFLICT",
        lambda: client.work_items.record_effect_receipt(
            tenant=TEST_TENANT,
            work_item_id=item,
            attempt=attempt,
            expected_revision=_view(client, item)["version"],
            receipt=_receipt(item, admitted["request_ref"], worker, "not_applied"),
            idempotency_key=stem + ":conflict",
        ),
    )
    assert (
        client.work_items.get_effect_receipt(
            tenant=TEST_TENANT, work_item_id=item, attempt=attempt
        )
        == receipt
    )


def _uncertain_run(client, stem: str) -> tuple[str, dict]:
    """Claim an item, admit its request, mark it started and record a receipt."""
    item = _item(client, stem)
    claim = _claim(client, item, stem)
    attempt = claim.get("attempt", 1)
    admitted = _admitted(b'{"call":"send"}')
    client.work_items.submit_admitted_request(
        work_item_id=item,
        attempt=attempt,
        expected_revision=_view(client, item)["version"],
        request=admitted,
    )
    started = client.work_items.mark_effect_started(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=claim["lease_holder_ref"],
        lease_epoch=claim["lease_epoch"],
        fencing_token=claim["fencing_token"],
        expected_revision=_view(client, item)["version"],
    )
    client.work_items.record_effect_receipt(
        tenant=TEST_TENANT,
        work_item_id=item,
        attempt=attempt,
        expected_revision=started["revision"],
        receipt=_receipt(item, admitted["request_ref"], claim["lease_holder_ref"]),
    )
    return item, claim


# spec: MEDULLA-DURABLE-KERNEL-R820.5
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R820.5")
def test_audit_and_metrics_after_uncertain_run(metered_client):
    client = metered_client
    before = _scrape(client)
    item, claim = _uncertain_run(client, uuid.uuid4().hex)
    attempt = claim.get("attempt", 1)

    lines = [line for line in _audit_lines(client) if item in line]
    assert len(lines) == 2, lines
    assert any("MarkWorkItemEffectStarted" in line for line in lines)
    assert any("RecordEffectReceipt" in line for line in lines)
    for line in lines:
        assert f"{item}:{attempt}:" in line
        assert "send" not in line  # no request parameters in the audit row

    after = _scrape(client)
    total = "medulla_work_item_uncertain_effects_total"
    for outcome in ("started", "applied"):
        series = f'{total}{{outcome="{outcome}"}}'
        assert after[series] == before.get(series, 0.0) + 1
    # The receipt closed the item the mark opened.
    assert after["medulla_work_item_uncertain_open"] == before.get(
        "medulla_work_item_uncertain_open", 0.0
    )


# spec: MEDULLA-DURABLE-KERNEL-R820.6
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R820.6")
def test_started_item_never_dead_letters(effect_client):
    client = effect_client
    stem = uuid.uuid4().hex
    item = _item(client, stem)
    # The item admits one attempt; the lease is short so it expires under us.
    claim = _claim(client, item, stem, lease_ms=1_500)
    attempt = claim.get("attempt", 1)
    admitted = _admitted(b'{"call":"send"}')
    client.work_items.submit_admitted_request(
        work_item_id=item,
        attempt=attempt,
        expected_revision=_view(client, item)["version"],
        request=admitted,
    )
    client.work_items.mark_effect_started(
        tenant=TEST_TENANT,
        work_item_id=item,
        worker_id=claim["lease_holder_ref"],
        lease_epoch=claim["lease_epoch"],
        fencing_token=claim["fencing_token"],
        expected_revision=_view(client, item)["version"],
    )

    time.sleep(2.0)  # the lease expires past the attempt ceiling
    reclaimed = _claim(client, item, stem, key="again")
    view = _view(client, item)
    assert view["effect_phase"] == "started"
    assert view["status"] != "dead_letter"
    assert reclaimed.get("attempt", attempt) == attempt

    client.work_items.record_effect_receipt(
        tenant=TEST_TENANT,
        work_item_id=item,
        attempt=attempt,
        expected_revision=view["version"],
        receipt=_receipt(item, admitted["request_ref"], reclaimed["lease_holder_ref"]),
    )
    assert _view(client, item)["effect_phase"] == "resolved"
