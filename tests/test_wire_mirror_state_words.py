"""Wire mirror field names that are not lifecycle states (organization constitution
Article XXXI census).

MEDULLA-FEDERATED-QUERY-R739.3: a `WITH PROOF` step names the kind of step by its
variant (`Sparql`, `Reason`, `Unproved`) and carries the admitting stage's canonical UQL
text as `clause`. Nothing moves a step, so no field is named `stage`.

MEDULLA-TYPED-PACKS-R811.1: the Python mirror of connector pack conformance evidence
types its state as the Rust enum and refuses any other word when decoded.
"""

from __future__ import annotations

import re
from pathlib import Path

import pydantic
import pytest
from pydantic import ValidationError

from medulla.generated import models
from medulla.generated.connector_pack import (
    ConformanceEvidence,
    ConformanceEvidenceState,
)

REPO = Path(__file__).resolve().parents[1]
RUST_OWNER = (
    REPO / "crates/medulla-catalog-types/src/connector_pack/conformance_evidence.rs"
)


@pytest.mark.no_engine
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R739.3")
def test_uql_proof_step_field_is_clause_not_stage() -> None:
    for body in (
        models.UqlProofStepReasonBody,
        models.UqlProofStepSparqlBody,
        models.UqlProofStepUnprovedBody,
    ):
        fields = set(body.model_fields)
        assert "clause" in fields, body.__name__
        assert "stage" not in fields, body.__name__

    step = models.UqlProofStepUnproved.model_validate(
        {"Unproved": {"clause": "MATCH (:Doc)"}}
    )
    assert step.Unproved.clause == "MATCH (:Doc)"
    # The old key is refused: there is no alias.
    with pytest.raises(pydantic.ValidationError):
        models.UqlProofStepUnproved.model_validate(
            {"Unproved": {"stage": "MATCH (:Doc)"}}
        )


def _rust_state_words() -> set[str]:
    source = RUST_OWNER.read_text(encoding="utf-8")
    body = re.search(r"pub enum ConformanceEvidenceState \{(.*?)\n\}", source, re.S)
    assert body, "Rust enum ConformanceEvidenceState moved or missing"
    variants = re.findall(r"^\s{4}([A-Z][A-Za-z0-9]*),", body.group(1), re.M)
    assert variants, "Rust enum ConformanceEvidenceState has no variants"
    return {re.sub(r"(?<!^)(?=[A-Z])", "_", v).lower() for v in variants}


def _record(state: str) -> dict[str, object]:
    return {
        "connector": "c",
        "expires_at": 2,
        "issued_at": 1,
        "pack_digest": "0" * 64,
        "revision": 1,
        "state": state,
        "tenant_id": "t",
        "tool_fingerprints": {},
    }


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R811.1")
def test_conformance_evidence_types_state_as_the_rust_enum() -> None:
    # spec: MEDULLA-TYPED-PACKS-R811.1
    assert (
        ConformanceEvidence.model_fields["state"].annotation is ConformanceEvidenceState
    )
    assert {member.value for member in ConformanceEvidenceState} == _rust_state_words()
    with pytest.raises(ValidationError):
        ConformanceEvidence.model_validate(_record("expired"))
    assert not hasattr(ConformanceEvidence, "apply")
