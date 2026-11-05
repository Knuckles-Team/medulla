"""Served association-job submission maps plain labels to opaque references."""

import json

import pytest

from medulla.client import EngineResponseError

DISTINCT_ITEM_LIMIT = 31


def _kind(labels: list[str]) -> dict:
    return {
        "MineAssociate": {
            "transactions": [labels],
            "min_support": 0.1,
            "min_confidence": 0.5,
            "algorithm": "fpgrowth",
        }
    }


@pytest.mark.spec("MEDULLA-PLACEMENT-R003.5")
def test_plain_labels_accepted_cap_enforced(clean_graph):
    labels = [f"plain label {i}" for i in range(DISTINCT_ITEM_LIMIT)]
    job = clean_graph.jobs.submit("association-submit", _kind(labels))
    assert job["job_id"]
    payload = job.get("input_payload")
    assert payload, "the durable job carries its opaque worker input"
    payload = bytes(payload)
    assert payload.count(b"eg:") >= DISTINCT_ITEM_LIMIT
    record = json.dumps(job, default=str).encode() + payload
    assert not any(label.encode() in record for label in labels)

    too_many = [*labels, "plain label overflow"]
    with pytest.raises(
        (EngineResponseError, RuntimeError),
        match="association job exceeds the distinct-item limit",
    ):
        clean_graph.jobs.submit("association-submit", _kind(too_many))
