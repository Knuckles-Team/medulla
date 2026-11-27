"""MEDULLA-DECISION-ENGINE-R833: the ``scale_intent`` Decide request shape."""

from __future__ import annotations

import json

import pytest

from medulla import decision_client as dc

_INTENT = {
    "intent_id": "i1",
    "intent_revision": 1,
    "expected_unit_revision": 3,
    "desired_replicas": 4,
    "min_replicas": 1,
    "max_replicas": 8,
    "controller_mode": "native",
    "replica_writer_id": "w1",
    "execution_key": "x1",
    "fence": {"lease_id": "l1", "lease_epoch": 2, "fence_token": 9},
    "target": {"workload_id": "u1"},
}


# spec: MEDULLA-DECISION-ENGINE-R833
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R833")
def test_scale_intent_request_shape() -> None:
    request = dc.scale_intent_request(
        "tenant:t", "fleet.scale:x1", "scale", _INTENT, {"schema": "s"}
    )
    assert request["question"] == {
        "question_id": "fleet.scale:x1",
        "kind": "scale_intent",
        "safety": "ordinary",
    }
    params = {p["name"]: p["value"] for p in request["params"]}
    assert params["graph"] == {"type": "text", "value": "scale"}
    assert json.loads(params["intent"]["value"])["desired_replicas"] == 4
    assert [o["option_id"] for o in request["candidates"]["options"]] == ["verdict"]
