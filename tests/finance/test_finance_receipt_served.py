"""R011.5 L4: a finance receipt moves only through its machine and lists by tenant key,
through the served engine and the Python client."""

from __future__ import annotations

import hashlib

import pytest


def _receipt_id(tenant_key: str, kind: str, key: str) -> str:
    digest = hashlib.sha256()
    for part in (tenant_key, kind, key):
        raw = part.encode()
        digest.update(len(raw).to_bytes(8, "big"))
        digest.update(raw)
    return digest.hexdigest()


def _receipt(key: str, state: str = "due") -> dict[str, object]:
    return {
        "type": "FinanceReceipt",
        "receiptKind": "alert_delivery",
        "receiptState": state,
        "tenantKey": "tenant-py",
        "idempotencyKey": key,
        "receiptEventId": "event-1",
        "receiptMode": "live",
        "receiptChannel": "email",
        "receiptAttempt": 1,
        "recordedAt": "2026-10-09T00:00:00Z",
    }


# spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
def test_receipt_machine_retry_and_list(clean_graph) -> None:
    node_id = _receipt_id("tenant-py", "alert_delivery", "py-key")
    assert clean_graph.nodes.create_if_absent(node_id, _receipt("py-key")) is True

    claimed = clean_graph.finance.receipt_transition(node_id, "claim")
    assert (claimed.receipt_state.value, claimed.attempt) == ("claimed", 1)
    failed = clean_graph.finance.receipt_transition(node_id, "fail")
    assert failed.receipt_state.value == "failed"
    retried = clean_graph.finance.receipt_transition(node_id, "retry")
    assert (retried.receipt_state.value, retried.attempt) == ("due", 2)

    with pytest.raises(Exception, match="TRANSITION_REFUSED"):
        clean_graph.finance.receipt_transition(node_id, "complete")
    with pytest.raises(Exception, match="RECEIPT_NOT_FOUND"):
        clean_graph.finance.receipt_transition("no-such-receipt", "claim")
    with pytest.raises(Exception, match="RECEIPT_STATE_INVALID"):
        other = _receipt_id("tenant-py", "alert_delivery", "py-late")
        clean_graph.nodes.create_if_absent(other, _receipt("py-late", "claimed"))

    page = clean_graph.finance.receipts_list("tenant-py")
    assert [r.receipt_id for r in page.receipts] == [node_id]
    assert page.receipts[0].attempt == 2
    assert clean_graph.finance.receipts_list("tenant-py", state="failed").receipts == []
    assert clean_graph.finance.receipts_list("tenant-other").receipts == []
