"""MEDULLA-DECISION-ENGINE-R615.7, R169.7: the queue listing and continuation mirror."""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla.generated import models

pytestmark = pytest.mark.no_engine

QUEUE = {
    "queue_id": "q:1",
    "status": "pending",
    "queued_by": "principal-a",
    "proposals": [
        {
            "id": "wp:1",
            "external_id": "ext:1",
            "system_of_record": "sor:ledger",
            "risk_tier": "high",
            "as_of_ms": 1_700_000_000_000,
        }
    ],
    "approvals": [
        {"proposal_id": "wp:1", "approver": "principal-b", "approved_at_ms": 5}
    ],
    "fully_approved": True,
}


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R615.7")
def test_queue_listing_round_trips() -> None:
    op = TypeAdapter(models.WritebackOp).validate_python(
        {"kind": "list", "queue_id": "q:1"}
    )
    assert isinstance(op, models.WritebackOpList)
    outcome = TypeAdapter(models.WritebackOutcome).validate_python(
        {"result": "listed", "queue": QUEUE}
    )
    assert isinstance(outcome, models.WritebackOutcomeListed)
    assert outcome.queue.status is models.QueueStatus.PENDING
    assert outcome.queue.fully_approved
    assert [p.id for p in outcome.queue.proposals] == ["wp:1"]
    with pytest.raises(ValidationError):
        models.QueueView.model_validate({**QUEUE, "status": "unknown"})
    with pytest.raises(ValidationError):
        models.WritebackOpList.model_validate({"kind": "list"})


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R169.7")
def test_continuation_read_round_trips() -> None:
    op = TypeAdapter(models.DecisionLogOp).validate_python(
        {"op": "feedback_continuation", "tenant_id": "tenant-a", "loop_id": "loop:1"}
    )
    assert isinstance(op, models.DecisionLogOpFeedbackContinuation)
    read = models.ContinuationRead.model_validate(
        {"continuation": {"loop_id": "loop:1", "round": 3, "stop": "round_bound"}}
    )
    assert read.continuation is not None
    assert read.continuation.stop is models.StopReason.ROUND_BOUND
    assert models.ContinuationRead.model_validate({}).continuation is None
