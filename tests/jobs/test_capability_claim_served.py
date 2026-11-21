"""A job whose required heavy capability no host declares is refused at submit."""

import uuid

import pytest

from medulla.client import EngineResponseError

KIND = {
    "MineAssociate": {
        "transactions": [["a", "b"]],
        "min_support": 0.1,
        "min_confidence": 0.5,
        "algorithm": "fpgrowth",
    }
}


@pytest.mark.spec("MEDULLA-PLACEMENT-R004.4")
def test_no_capable_host_refuses_at_submit(clean_graph):
    graph = f"capability-claim-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph)
    # No host of this engine declares `video_transcode`: the submit is refused by name.
    with pytest.raises(
        (EngineResponseError, RuntimeError),
        match="NO_CAPABLE_HOST: video_transcode",
    ):
        clean_graph.jobs.submit(
            graph, KIND, required_capabilities=["video_transcode"]
        )
    # Nothing was queued, and a claim never falls back to another host.
    slot = f"slot-{uuid.uuid4().hex}"
    assert clean_graph.jobs.worker_claim(slot, []) is None
