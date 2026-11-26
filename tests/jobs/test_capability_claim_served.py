"""A job whose required heavy capability no host declares is refused at submit."""

import os
import uuid

import pytest
from conftest import register_commons_agents, request_context

from medulla.client import EngineResponseError, SyncMedullaClient

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
        clean_graph.jobs.submit(graph, KIND, required_capabilities=["video_transcode"])
    # Nothing was queued, and a claim never falls back to another host.
    slot = f"slot-{uuid.uuid4().hex}"
    assert clean_graph.jobs.worker_claim(slot, []) is None


HOST_CLASS = "https://knuckles-team.github.io/ontology/placement#Host"
HOST_CAPABILITIES = "https://knuckles-team.github.io/ontology/placement#capabilities"


def _declare_host(host_iri, capabilities):
    """Write one ``placement:Host`` node to the commons graph (served write path)."""
    register_commons_agents(request_context()["agent_id"])
    commons = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph="__commons__",
    )
    try:
        commons.nodes.add(
            host_iri, {"type": HOST_CLASS, HOST_CAPABILITIES: list(capabilities)}
        )
    finally:
        commons.close()


def _submit_backtest(clean_graph):
    graph = f"capability-claim-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph)
    return clean_graph.jobs.submit(graph, KIND, required_capabilities=["backtest"])


@pytest.mark.spec("MEDULLA-PLACEMENT-R004.6")
def test_backtest_job_lands_on_declared_host(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    alpha, beta = f"host:alpha-{suffix}", f"host:beta-{suffix}"
    _declare_host(alpha, ["backtest"])
    _declare_host(beta, [])
    job = _submit_backtest(clean_graph)
    # `beta` declares nothing: its worker claims nothing.
    beta_worker = f"worker-beta-{suffix}"
    clean_graph.jobs.worker_register(beta_worker, beta)
    assert clean_graph.jobs.worker_claim(beta_worker, []) is None
    # `alpha` declares `backtest`: its worker claims the job.
    alpha_worker = f"worker-alpha-{suffix}"
    clean_graph.jobs.worker_register(alpha_worker, alpha)
    claim = clean_graph.jobs.worker_claim(alpha_worker, [])
    assert claim is not None
    assert claim["job"]["job_id"] == job["job_id"]


@pytest.mark.spec("MEDULLA-PLACEMENT-R004.5")
def test_routing_record_names_candidates_and_generation(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    alpha = f"host:alpha-{suffix}"
    _declare_host(alpha, ["backtest"])
    job = _submit_backtest(clean_graph)
    worker = f"worker-alpha-{suffix}"
    clean_graph.jobs.worker_register(worker, alpha)
    claim = clean_graph.jobs.worker_claim(worker, [])
    assert claim is not None
    status = clean_graph.jobs.status(job["job_id"])
    routing = status["routing"]
    assert len(routing) == 1
    assert routing[0]["required_capability"] == "backtest"
    assert alpha in routing[0]["candidates"]
    assert routing[0]["chosen"] == alpha
    assert routing[0]["topology_generation"] > 0
