"""MEDULLA-SDLC-R031: the Python mirror of the defect and lifecycle request shapes."""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla.generated import METHOD_IDS, SEND_BY_METHOD, models

pytestmark = pytest.mark.no_engine

LIFECYCLE_METHODS = (
    "FileDefect",
    "TransitionDefect",
    "GetDefect",
    "TriageDefect",
    "SpecStatus",
    "LifecycleTrace",
)


@pytest.mark.spec("MEDULLA-SDLC-R031")
def test_every_lifecycle_method_has_an_id_a_sender_and_a_method_variant() -> None:
    adapter = TypeAdapter(models.Method)
    for method in LIFECYCLE_METHODS:
        assert method in METHOD_IDS
        assert method in SEND_BY_METHOD
        assert hasattr(models, f"Method{method}Params")
    parsed = adapter.validate_python(
        {
            "method": "FileDefect",
            "params": {
                "request": {
                    "repo": "r",
                    "intake": "gate_failure",
                    "title": "t",
                    "source_record_id": "verdict:1",
                    "observed_at": 5,
                    "text": "panic",
                }
            },
        }
    )
    assert isinstance(parsed, models.MethodFileDefect)
    assert parsed.params.request.severity is None


@pytest.mark.spec("MEDULLA-SDLC-R031")
def test_the_request_and_answer_shapes_match_the_wire() -> None:
    move = models.TransitionDefectRequest.model_validate(
        {"repo": "r", "defect_id": "def-0001", "to": "fixed", "fix_changeset": "c1"}
    )
    assert move.to is models.DefectStatus.FIXED
    assert move.gate_id is None and move.reason is None
    receipt = models.TransitionDefectReceipt.model_validate(
        {"node_id": "defect:r/def-0001", "from": "new", "to": "triaged"}
    )
    assert receipt.from_ is models.DefectStatus.NEW
    status = models.SpecStatusAnswer.model_validate(
        {
            "repo": "r",
            "rev": "c1",
            "rows": [
                {
                    "id": "X-R001",
                    "state": "LANDED",
                    "landed_by": ["c1"],
                    "verified_by": [],
                    "withdrawn": False,
                }
            ],
        }
    )
    assert status.rows[0].state is models.SpecState.LANDED
    trace = models.LifecycleTraceAnswer.model_validate(
        {
            "records": [{"id": "defect:r/def-0001", "kind": "defect"}],
            "edges": [
                {
                    "from": "defect:r/def-0001",
                    "to": "commit:c1",
                    "kind": "introducedBy",
                    "rule": "introduced_by",
                    "rule_version": 1,
                }
            ],
            "truncated": False,
        }
    )
    assert trace.policy_digest == ""
    assert trace.edges[0].kind is models.LifecycleEdgeKind.INTRODUCED_BY
    request = models.LifecycleTraceRequest.model_validate({"anchor": "a", "depth": 2})
    assert request.kinds == [] and request.policy is None
    with pytest.raises(ValidationError):
        models.FileDefectRequest.model_validate(
            {
                "repo": "r",
                "intake": "email",
                "title": "t",
                "source_record_id": "s",
                "observed_at": 1,
                "text": "x",
            }
        )
    with pytest.raises(ValidationError):
        models.MethodSpecStatusParams.model_validate({"repo": "r", "rev": "c1"})
