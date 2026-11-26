"""MEDULLA-DECISION-ENGINE-R824.1: the Python mirror of the capacity proposal shapes."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated import models

pytestmark = pytest.mark.no_engine

PROPOSAL = {
    "proposal_id": "p-1",
    "tenant_id": "tenant-a",
    "resource_class_iri": "https://example.test/ns#Gpu",
    "unit": "gpu_slot",
    "current_capacity": 4,
    "proposed_capacity": 6,
    "min_capacity": 1,
    "max_capacity": 8,
    "measured_load": 3,
    "available_headroom": 4,
    "window_start_ms": 1000,
    "window_end_ms": 2000,
    "premise_revision": 7,
    "policy_revision": 2,
    "proposal_digest": "a" * 64,
}


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R824.1")
def test_the_proposal_and_the_observation_mirror_the_wire_shape() -> None:
    proposal = models.CapacityAdjustmentProposal.model_validate(PROPOSAL)
    assert proposal.model_dump() == PROPOSAL
    observation = models.Observation.model_validate(
        {"unit": "gpu_slot", "available": 5, "epoch": 7}
    )
    assert (observation.unit, observation.available, observation.epoch) == (
        "gpu_slot",
        5,
        7,
    )
    with pytest.raises(ValidationError):
        models.CapacityAdjustmentProposal.model_validate({**PROPOSAL, "extra": 1})
    with pytest.raises(ValidationError):
        models.CapacityAdjustmentProposal.model_validate(
            {**PROPOSAL, "proposed_capacity": -1}
        )
    with pytest.raises(ValidationError):
        models.Observation.model_validate({"unit": "gpu_slot", "available": 5})


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R824.1")
def test_the_refusal_codes_are_the_nine_engine_codes_in_check_order() -> None:
    assert [code.value for code in models.CapacityRefusalCode] == [
        "CAPACITY_PROPOSAL_DIGEST_INVALID",
        "CAPACITY_TENANT_MISMATCH",
        "CAPACITY_PREMISE_STALE",
        "CAPACITY_MEASUREMENT_INVALID",
        "CAPACITY_UNIT_MISMATCH",
        "CAPACITY_WINDOW_INVALID",
        "CAPACITY_LIMIT_INVALID",
        "CAPACITY_LIMIT_EXCEEDED",
        "CAPACITY_HEADROOM_EXCEEDED",
    ]
