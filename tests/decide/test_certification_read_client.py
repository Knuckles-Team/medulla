"""MEDULLA-DECISION-ENGINE-R142.103 wire mirror of the certification read.

L1: the op builder of ``medulla.decision_client`` and the generated models agree with
the engine's wire shape, with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla import decision_client as dc
from medulla.generated import models


# spec: MEDULLA-DECISION-ENGINE-R142.103
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R142.103")
def test_the_certification_op_validates_against_the_generated_models() -> None:
    adapter = TypeAdapter(models.DecisionEvalOp)
    op = dc.certification_op("t", "skill-a", "sha256:aa")
    assert op["op"] == "certification"
    parsed = adapter.validate_python(op)
    assert isinstance(parsed, models.DecisionEvalOpCertification)
    assert parsed.request.component_id == "skill-a"
    bad = dc.certification_op("t", "skill-a", "sha256:aa")
    bad["request"]["extra"] = 1
    with pytest.raises(ValidationError):
        adapter.validate_python(bad)
    answer = models.CertificationReadAnswer.model_validate({"attestation": None})
    assert answer.attestation is None
