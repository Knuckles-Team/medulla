"""MEDULLA-UNIFIED-DATA-PLANE-R762.3: approved quality rules scored by a job.

`QualityRulePut` stores an approved rule under the verified tenant. Each
`QualityEvaluate` job scores it; a streak of failures opens one incident that
`IncidentEvidence` reads back under the same tenant.
"""

from __future__ import annotations

import os
import time
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT, request_context


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _evaluate(client: Any, graph: str, target: str) -> list[dict]:
    job = client.jobs.submit(graph, {"QualityEvaluate": {"target": target}})
    for _ in range(300):
        status = client.jobs.status(job["job_id"])
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            assert name == "Succeeded", f"quality job ended {state!r}"
            return status["output"]["rows"]
        time.sleep(0.1)
    raise AssertionError("quality job did not finish")


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.3")
def test_rule_scores_and_incident(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.3: scores, streak, incident."""
    client = _client()
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    table = f"orders_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client.query.sql(f"CREATE TABLE {table} (id int primary key, status text)")
        client.query.sql(f"INSERT INTO {table} VALUES (1, NULL), (2, NULL)")
        rule = {
            "id": "status-complete",
            "target": table,
            "dimension": "completeness",
            "expression": "status IS NOT NULL",
            "threshold": 0.9,
            "severity": "critical",
        }
        receipt = _decode(client._send("QualityRulePut", {"rule": rule}, graph))
        assert receipt["rule_id"] == "status-complete"
        assert receipt["state"]["state"] == "approved"

        streak = [_evaluate(client, graph, table)[0] for _ in range(3)]
        assert [row["passed"] for row in streak] == [False, False, False]
        assert [row["consecutive_failures"] for row in streak] == [1, 2, 3]
        [incident_id] = streak[-1]["incident_ids"]

        record = _decode(
            client._send("IncidentEvidence", {"incident_id": incident_id}, graph)
        )
        assert record["tenant"] == TEST_TENANT

        with pytest.raises(Exception, match="QUALITY_RULE_INVALID"):
            client._send(
                "QualityRulePut",
                {"rule": {**rule, "expression": "1; DROP TABLE x"}},
                graph,
            )
    finally:
        client.tenants.delete(graph)
        client.close()
