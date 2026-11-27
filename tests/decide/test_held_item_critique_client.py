"""MEDULLA-DECISION-ENGINE-R038.6 wire mirror of the held-item critique.

L1: the op builder of ``medulla.decision_client`` and the generated models agree with
the engine's wire shapes, with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla import decision_client as dc
from medulla.generated import models

ANSWERS = [
    {"id": "grounded", "passed": True},
    {"id": "complete", "passed": True},
    {"id": "scoped", "passed": False},
    {"id": "safe", "passed": True},
]


# spec: MEDULLA-DECISION-ENGINE-R038.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R038.6")
def test_the_critique_op_and_the_checklist_answer_mirror_the_wire() -> None:
    adapter = TypeAdapter(models.DecisionEvalOp)
    op = dc.held_item_critique_op("t", "held", "prop-1", "b" * 64, ANSWERS)
    assert op["op"] == "held_item_critique"
    parsed = adapter.validate_python(op)
    assert isinstance(parsed, models.DecisionEvalOpHeldItemCritique)
    assert [answer.id for answer in parsed.request.answers] == [
        "grounded",
        "complete",
        "scoped",
        "safe",
    ]
    bad = dc.held_item_critique_op("t", "held", "prop-1", "b" * 64, ANSWERS)
    bad["request"]["extra"] = 1
    with pytest.raises(ValidationError):
        adapter.validate_python(bad)

    answer = models.HeldItemAnswer.model_validate(
        {
            "outcome": "abstained",
            "verdict_ref": "rec-1",
            "reason": "CRITIQUE_MISSING",
            "checklist": [{"id": "grounded", "question": "Is it grounded?"}],
        }
    )
    assert answer.checklist[0].id == "grounded"
    assert answer.critique_digest is None
    recorded = models.HeldItemCritiqueRecorded.model_validate(
        {
            "critique_ref": "sha256:aa",
            "item_ref": "prop-1",
            "content_digest": "b" * 64,
            "verdict": "reject",
        }
    )
    assert recorded.verdict == "reject"
