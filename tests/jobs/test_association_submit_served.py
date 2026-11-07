"""Served association-job submission maps plain labels to opaque references."""

import json
import uuid

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
    # A job is anchored to an existing graph of the caller.
    graph = f"association-submit-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph)
    labels = [f"plain label {i}" for i in range(DISTINCT_ITEM_LIMIT)]
    # A unique capability keeps the server's own executor from claiming the job,
    # so this test claims it as the worker and reads the input it is handed.
    capability = f"association-submit-{uuid.uuid4().hex}"
    job = clean_graph.jobs.submit(
        graph, _kind(labels), required_capabilities=[capability]
    )
    assert job["job_id"]
    claimed = clean_graph.jobs.worker_claim(f"slot-{capability}", [capability])
    assert claimed is not None
    assert claimed["job"]["job_id"] == job["job_id"]
    payload = claimed["job"].get("input_payload")
    assert payload, "the durable job carries its opaque worker input"
    payload = bytes(payload)
    assert payload.count(b"eg:") >= DISTINCT_ITEM_LIMIT
    record = json.dumps(job, default=str).encode() + payload
    assert not any(label.encode() in record for label in labels)
    # Cancel and confirm as the worker, so the job holds no tenant active slot.
    clean_graph.jobs.cancel(job["job_id"])
    clean_graph.jobs.worker_cancel(
        job["job_id"], f"slot-{capability}", claimed["lease"]["epoch"]
    )

    too_many = [*labels, "plain label overflow"]
    with pytest.raises(
        (EngineResponseError, RuntimeError),
        match="association job exceeds the distinct-item limit",
    ):
        clean_graph.jobs.submit(graph, _kind(too_many))
