"""Wire mirrors carry a machine's state as the closed enum, never as free text.

The Rust machines own the state words (MEDULLA-DURABLE-KERNEL-R874); the hand-kept
Python mirrors decode them and refuse a word outside the enum.
"""

from __future__ import annotations

import enum

import pytest
from pydantic import ValidationError

from medulla.generated import models

pytestmark = [pytest.mark.no_engine]

STAGE_MIRRORS = (
    "SemanticIndexErrorStageScopeMismatch",
    "SemanticGenerationCheckpoint",
    "SemanticGenerationDependencyCheckpoint",
)


def _annotation(model: type, field: str) -> object:
    return model.model_fields[field].annotation


# spec: MEDULLA-DURABLE-KERNEL-R875.6
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R875.6")
def test_semantic_and_audit_mirrors_type_state_as_the_rust_enum() -> None:
    assert _annotation(models.MethodAuditAppendParams, "status") is models.AuditStatus
    assert {m.value for m in models.AuditStatus} == {
        "reserved",
        "ok",
        "error",
        "denied",
    }
    for name in STAGE_MIRRORS:
        assert _annotation(getattr(models, name), "stage") is models.SemanticStage, name
    status = _annotation(models.SemanticOutboxStatus, "status")
    assert isinstance(status, type) and issubclass(status, enum.Enum)

    fields = {
        "op": "graph.nodes.write",
        "params_sha256": "a" * 64,
        "request_id": "request-1",
        "surface": "http",
    }
    accepted = models.MethodAuditAppendParams(status="reserved", **fields)
    assert accepted.status is models.AuditStatus.RESERVED
    with pytest.raises(ValidationError):
        models.MethodAuditAppendParams(status="closed", **fields)
