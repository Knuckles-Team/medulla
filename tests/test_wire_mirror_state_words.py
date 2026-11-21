"""Wire mirrors carry a machine's state as the closed enum, never as free text.

The Rust machines own the state words (MEDULLA-DURABLE-KERNEL-R874); the hand-kept
Python mirrors decode them and refuse a word outside the enum.
"""

from __future__ import annotations

import enum

import pytest
from pydantic import BaseModel, ValidationError

from medulla.generated import models

pytestmark = [pytest.mark.no_engine]

STAGE_MIRRORS = (
    "SemanticIndexErrorStageScopeMismatch",
    "SemanticGenerationCheckpoint",
    "SemanticGenerationDependencyCheckpoint",
)


def _annotation(model: type[BaseModel], field: str) -> object:
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
    # The outbox consumer standing is a snapshot of counters, not a lifecycle
    # (MEDULLA-DURABLE-KERNEL-R874.11): it is a model under `standing`, and the
    # old `status` field no longer exists.
    standing = _annotation(models.SemanticOutboxStatus, "standing")
    assert standing is models.OutboxConsumerStanding
    assert "status" not in models.SemanticOutboxStatus.model_fields
    assert "status" not in models.MutationOutboxStatusView.model_fields

    fields = {
        "op": "graph.nodes.write",
        "params_sha256": "a" * 64,
        "request_id": "request-1",
        "surface": "http",
    }
    accepted = models.MethodAuditAppendParams.model_validate(
        {"status": "reserved", **fields}
    )
    assert accepted.status is models.AuditStatus.RESERVED
    with pytest.raises(ValidationError):
        models.MethodAuditAppendParams.model_validate({"status": "closed", **fields})


# spec: MEDULLA-ORCH-VOCAB-R816.1
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R816.1")
def test_incident_set_status_params_type_status_as_the_rust_enum() -> None:
    assert (
        _annotation(models.MethodIncidentSetStatusParams, "status")
        is models.IncidentStatus
    )
    assert issubclass(models.IncidentStatus, enum.Enum)
    assert {"open", "acknowledged", "resolved"} <= {
        m.value for m in models.IncidentStatus
    }
    accepted = models.MethodIncidentSetStatusParams.model_validate(
        {"incident_id": "inc-1", "status": "acknowledged"}
    )
    assert accepted.status is models.IncidentStatus.ACKNOWLEDGED
    with pytest.raises(ValidationError):
        models.MethodIncidentSetStatusParams.model_validate(
            {"incident_id": "inc-1", "status": "skipped"}
        )


# spec: MEDULLA-DECISION-ENGINE-R831.10
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R831.10")
def test_harness_part_tag_is_decision_not_status() -> None:
    parts = (
        "HarnessPart_ActionSpace",
        "HarnessPart_BoundedVec_string_256",
        "HarnessPart_ContextPolicy",
        "HarnessPart_PlanScaffold",
        "HarnessPart_UnitRationalWire",
    )
    for name in parts:
        for variant in ("Decided", "Abstained"):
            model = getattr(models, name + variant)
            assert "decision" in model.model_fields, name + variant
            assert "status" not in model.model_fields, name + variant

    decided = models.HarnessPart_ActionSpaceDecided.model_validate(
        {"decision": "decided", "value": "tool_set"}
    )
    assert decided.decision == "decided"
    abstained = models.HarnessPart_ActionSpaceAbstained.model_validate(
        {
            "decision": "abstained",
            "missing_fact": ["unverified"],
            "conservative_default": "tool_set",
        }
    )
    assert abstained.decision == "abstained"
    # The old key is a hard cutover: no alias is accepted.
    with pytest.raises(ValidationError):
        models.HarnessPart_ActionSpaceDecided.model_validate(
            {"status": "decided", "value": "tool_set"}
        )


# spec: MEDULLA-TYPED-PACKS-R811.1
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R811.1")
def test_conformance_evidence_types_state_as_the_rust_enum() -> None:
    from medulla.generated import connector_pack

    evidence = connector_pack.ConformanceEvidence
    state = connector_pack.ConformanceEvidenceState
    # The mirror types the state as the closed enum, never as `str` or free text.
    assert _annotation(evidence, "state") is state
    assert issubclass(state, enum.Enum)
    assert {member.value for member in state} == {"active", "revoked"}
    # The mirror holds no transition function: the Rust machine is the only writer.
    assert not any(
        name in vars(evidence) for name in ("apply", "revoke", "transition")
    )

    fields = {
        "connector": "conn",
        "expires_at": 20,
        "issued_at": 10,
        "pack_digest": "a" * 64,
        "revision": 1,
        "tenant_id": "tenant",
        "tool_fingerprints": {"tool://conn/search": "b" * 64},
    }
    accepted = evidence.model_validate({"state": "active", **fields})
    assert accepted.state is state.ACTIVE
    revoked = evidence.model_validate({"state": "revoked", "revoked_at": 15, **fields})
    assert revoked.state is state.REVOKED
    # A word outside the enum is refused when decoded.
    for word in ("expired", "Active", "", "closed"):
        with pytest.raises(ValidationError):
            evidence.model_validate({"state": word, **fields})
