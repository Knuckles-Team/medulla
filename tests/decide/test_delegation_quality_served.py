"""MEDULLA-DECISION-ENGINE-R157.6 served delegation quality gate: windows of
observations above and below the threshold are submitted to `DecisionEval`
over the wire and the sealed admission is read from the answer."""

from __future__ import annotations

import hashlib
import time
from typing import Any

import pytest
from conftest import TEST_TENANT

from medulla import decision_client as dc

HOUR_MS = 3_600_000
THRESHOLD = 800_000


def _observation(scope: str, n: int, score: int, start_ms: int) -> dict[str, Any]:
    observation_id = f"{scope}-{n}"
    return {
        "observation_id": observation_id,
        "score": score,
        "observed_at_ms": start_ms + 10 + n,
        "evidence_digest": hashlib.sha256(observation_id.encode()).hexdigest(),
    }


def _request(scope: str, scores: list[int]) -> dict[str, Any]:
    end_ms = int(time.time() * 1000) - 10_000
    start_ms = end_ms - 2 * HOUR_MS
    return {
        "tenant_id": TEST_TENANT,
        "scope": scope,
        "policy": {"policy": "default"},
        "window": {
            "start_ms": start_ms,
            "end_ms": end_ms,
            "observation_count": len(scores),
        },
        "observations": [
            _observation(scope, n, score, start_ms) for n, score in enumerate(scores)
        ],
        "threshold": {
            "metric_name": "task_success",
            "minimum_value": THRESHOLD,
            "min_observations": 3,
        },
    }


def _submit(client: Any, request: dict[str, Any]) -> dict[str, Any]:
    return client._send("DecisionEval", {"op": dc.delegation_quality_op(request)})


# spec: MEDULLA-DECISION-ENGINE-R157.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R157.6")
def test_quality_gate_admits_and_refuses(clean_graph):
    client = clean_graph

    admitted = _submit(client, _request("agent-up", [900_000, 800_000, 850_000]))
    admission = admitted["stored"]["admission"]
    assert admitted["replaced"] is True
    assert admission["admitted"] is True
    assert admission["quality_value"] == 850_000
    assert admitted["stored"]["scope"] == "agent-up"
    assert len(admitted["artifact_digest"]) == 64

    refused = _submit(client, _request("agent-down", [500_000, 600_000, 700_000]))
    admission = refused["stored"]["admission"]
    assert admission["admitted"] is False
    assert admission["reason"] == "DELEGATION_BELOW_THRESHOLD"
    assert admission["quality_value"] == 600_000
    assert refused["artifact_digest"] != admitted["artifact_digest"]
