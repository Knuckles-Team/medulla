"""The Python mirror of connector pack conformance evidence types its state as
the Rust enum and refuses any other word when decoded."""

from __future__ import annotations

import re
from pathlib import Path

import pytest
from pydantic import ValidationError

from medulla.generated.connector_pack import (
    ConformanceEvidence,
    ConformanceEvidenceState,
)

REPO = Path(__file__).resolve().parents[1]
RUST_OWNER = (
    REPO / "crates/medulla-catalog-types/src/connector_pack/conformance_evidence.rs"
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
