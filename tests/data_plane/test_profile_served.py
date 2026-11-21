"""MEDULLA-UNIFIED-DATA-PLANE-R761.3: a served dataset profile job.

A `DatasetProfile` job profiles a native table under the verified tenant and
writes one tenant-private `ColumnProfile` node per column, linked to the
catalog column it describes. A second tenant reads none of it.
"""

from __future__ import annotations

import os
import time
import uuid
from typing import Any

import pytest
from conftest import request_context


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _wait(client: Any, job_id: str) -> dict:
    status: dict = {}
    for _ in range(300):
        status = client.jobs.status(job_id)
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            assert name == "Succeeded", f"profile job ended {state!r}"
            return status
        time.sleep(0.1)
    raise AssertionError(f"profile job did not finish: {status!r}")


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R761.3")
def test_profile_job_attaches_to_catalog(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R761.3: profile rows per column."""
    client = _client()
    graph = f"profile-{uuid.uuid4().hex[:8]}"
    table = f"orders_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client.query.sql(f"CREATE TABLE {table} (id int primary key, status text)")
        client.query.sql(
            f"INSERT INTO {table} VALUES (1, 'open'), (2, 'open'), (3, NULL)"
        )
        job = client.jobs.submit(
            graph, {"DatasetProfile": {"dataset": table, "columns": ["status"]}}
        )
        status = _wait(client, job["job_id"])
        rows = status["output"]["rows"]
        assert [row["column"] for row in rows] == ["status"]
        assert rows[0]["dataset"] == table
        assert rows[0]["kind"] == "column_profile"
        assert rows[0]["rows"] == 3

        with pytest.raises(Exception, match="PROFILE_UNSUPPORTED|INVALID"):
            client.jobs.submit(graph, {"DatasetProfile": {"dataset": "bad name;"}})
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R761.4")
def test_profile_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R761.4: both refusals come at submit."""
    client = _client()
    graph = f"profile-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        before = client.nodes.count()
        # A dataset the caller's tenant does not hold (another tenant's table refuses the
        # same way) is forbidden at submit: no job id comes back.
        with pytest.raises(Exception, match="PROFILE_FORBIDDEN"):
            client.jobs.submit(
                graph,
                {
                    "DatasetProfile": {
                        "dataset": f"nobody_{uuid.uuid4().hex[:8]}",
                        "columns": ["id"],
                    }
                },
            )
        # A registered source that takes neither a pushed aggregate nor a sample is
        # unsupported at submit.
        source = f"profile-mcp-{uuid.uuid4().hex[:8]}"
        client._send(
            "RegisterForeignSource",
            {
                "name": source,
                "source": {"Mcp": {"server": "profile-test-mcp", "tool": "search"}},
            },
            None,
            idempotency_key=f"register-{source}",
        )
        with pytest.raises(Exception, match="PROFILE_UNSUPPORTED"):
            client.jobs.submit(
                graph,
                {"DatasetProfile": {"dataset": f"{source}.items", "columns": ["id"]}},
            )
        # Neither refusal wrote a node.
        assert client.nodes.count() == before
    finally:
        client.tenants.delete(graph)
        client.close()
