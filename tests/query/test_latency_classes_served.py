"""Query latency classes over the served wire (MEDULLA-FEDERATED-QUERY-R731).

One engine runs with ``query.interactive_node_budget`` at its registry floor of
1,000 over a 5,000-node fixture. An interactive ``UnifiedQuery`` and ``Sparql``
refuse ``QUERY_BUDGET_EXCEEDED`` with the hint to resubmit as analytical; the
same queries as ``analytical`` answer a job record, and ``AnalyticsJob``
``Status`` returns, once published, rows equal to the unbounded run. A second
tenant's ``Status`` on that id is refused.
"""

from __future__ import annotations

import secrets
import time

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    server_timeout,
    spawn_server,
    stop_server,
    strict_server_env,
    tenant_request_context,
    wait_for_server,
)

from medulla.client import EngineResponseError, SyncMedullaClient

_BUDGET = 1_000
_NODES = 5_000
_BATCH = 200
_PLAN = [{"Scan": {"label": "Wide"}}]
_SPARQL = "SELECT ?s WHERE { ?s ?p ?o }"


@pytest.fixture(scope="module")
def budgeted_engine(tmp_path_factory):
    """One engine with the interactive node budget at its floor."""
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    runtime = tmp_path_factory.mktemp("latency-classes")
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir()
    log_path = runtime / "engine.log"
    secret = secrets.token_hex(32)
    env = strict_server_env(
        str(runtime / "security"), auth_secret=secret, persist_dir=str(persist)
    )
    process = spawn_server(
        [
            binary,
            "--socket-path",
            socket_path,
            "--knob",
            f"query.interactive_node_budget={_BUDGET}",
        ],
        env=env,
        log_path=log_path,
    )
    try:
        wait_for_server(
            process, socket_path, name="latency class engine", log_path=log_path
        )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=secret,
            verified_context=bootstrap_context(),
        ) as bootstrap:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=secret,
            verified_context=request_context(),
            timeout=server_timeout(),
        ) as client:
            yield client, socket_path, secret
    finally:
        stop_server(process, name="latency class engine", log_path=log_path)


def _wait_published(client, job_id: str) -> dict:
    deadline = time.monotonic() + server_timeout()
    while time.monotonic() < deadline:
        job = client.jobs.status(job_id)
        if job.get("output") is not None:
            return job
        time.sleep(0.25)
    raise AssertionError(f"job {job_id} did not publish")


# spec: MEDULLA-FEDERATED-QUERY-R731
def test_interactive_budget_refuses_and_analytical_runs_as_job(budgeted_engine):
    client, socket_path, secret = budgeted_engine
    client.graph.clear()
    ids = [f"wide-{i:05d}" for i in range(_NODES)]
    for start in range(0, len(ids), _BATCH):
        client.lifecycle.batch_update(
            [
                {"op": "add_node", "id": node, "properties": {"type": "Wide"}}
                for node in ids[start : start + _BATCH]
            ]
        )

    # An interactive query past the bound refuses with the hint, for both methods.
    with pytest.raises(EngineResponseError) as refused:
        client.query.unified(_PLAN)
    assert "QUERY_BUDGET_EXCEEDED" in str(refused.value)
    assert "latency_class analytical" in str(refused.value)
    with pytest.raises(EngineResponseError) as refused:
        client.rdf.sparql(_SPARQL)
    assert "QUERY_BUDGET_EXCEEDED" in str(refused.value)

    # The same queries as analytical answer a durable job record.
    unified_job = client.query.unified(_PLAN, latency_class="analytical")
    sparql_job = client.rdf.sparql(_SPARQL, latency_class="analytical")
    assert unified_job["job_id"] and sparql_job["job_id"]

    published = _wait_published(client, unified_job["job_id"])
    rows = published["output"]["rows"]
    assert sorted(row["id"] for row in rows) == ids
    assert len(_wait_published(client, sparql_job["job_id"])["output"]["rows"]) > 0

    # A second tenant is refused the first tenant's job.
    with SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=secret,
        verified_context=tenant_request_context("tenant-other"),
        timeout=server_timeout(),
    ) as other:
        with pytest.raises(EngineResponseError):
            other.jobs.status(unified_job["job_id"])
