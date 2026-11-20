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
from _harness_envelope import draft_envelope, op
from conftest import TEST_TENANT, request_context

FOREIGN_TENANT = "tenant:foreign"


def _client(tenant: str | None = None) -> Any:
    from medulla.client import SyncMedullaClient

    context = request_context()
    if tenant is not None:
        context["tenant"] = tenant
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
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


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762")
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


SHAPE_IRI = "https://knuckles-team.github.io/ontology/quality#CustomerShape"
CUSTOMER_SHAPES = """@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix q: <https://knuckles-team.github.io/ontology/quality#> .
q:CustomerShape a sh:NodeShape ;
    sh:property [ sh:path q:email ; sh:minCount 1 ] .
"""
# The freshness bound of the rule: one hour.
MAX_LAG_MS = 3_600_000


def _add_customer(client: Any, graph: str, node_id: str, email: str | None) -> None:
    properties: dict[str, Any] = {"type": "Customer", "name": node_id}
    if email is not None:
        properties["email"] = email
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(properties)},
        graph,
    )


def _capture(client: Any, graph: str, source: str, at_ms: int) -> None:
    """One governed change whose source cursor stands at ``at_ms``."""
    node_id = f"feed:{uuid.uuid4().hex[:8]}"
    body = {"type": "Document", "title": node_id}
    envelope = draft_envelope(
        graph,
        node_id,
        f"quality-feed:{node_id}",
        1,
        [
            op(
                0,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(body)},
            )
        ],
    )
    envelope["cursor"] = {
        "source": source,
        "partition": "",
        "position": {"kind": "timestamp_millis", "value": at_ms},
    }
    client.changes.apply(envelope)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.9")
def test_shape_and_freshness_scored(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.9: shape and freshness scored."""
    client = _client()
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    source = f"feed_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client._send(
            "GraphSchema",
            {
                "op": {
                    "op": "attach",
                    "source_id": "quality-customer-shapes",
                    "shapes_ttl": CUSTOMER_SHAPES,
                }
            },
            graph,
            idempotency_key=f"attach-{uuid.uuid4().hex}",
        )
        _add_customer(client, graph, "cust-1", "one")
        _add_customer(client, graph, "cust-2", None)
        shape_rule = {
            "id": "customer-shape",
            "target": "graph:Customer",
            "dimension": "validity",
            "expression": SHAPE_IRI,
            "threshold": 0.9,
            "severity": "critical",
        }
        fresh_rule = {
            "id": "feed-fresh",
            "target": source,
            "dimension": "freshness",
            "expression": str(MAX_LAG_MS),
            "threshold": 1.0,
            "severity": "critical",
        }
        for rule in (shape_rule, fresh_rule):
            receipt = _decode(client._send("QualityRulePut", {"rule": rule}, graph))
            assert receipt["rule_id"] == rule["id"]

        [shape] = _evaluate(client, graph, "graph:Customer")
        assert shape["rule"] == "customer-shape"
        assert shape["code"] is None, shape
        assert (shape["rows_checked"], shape["rows_failed"]) == (2, 1)
        assert shape["value"] == 0.5
        assert shape["passed"] is False

        # No change captured yet: the rule is unscored, never guessed fresh.
        [unmeasured] = _evaluate(client, graph, source)
        assert unmeasured["code"] == "QUALITY_MEASURE_UNAVAILABLE"
        assert unmeasured["value"] is None

        _capture(client, graph, source, int(time.time() * 1000) - 1_000)
        [fresh] = _evaluate(client, graph, source)
        assert fresh["code"] is None, fresh
        assert fresh["value"] == 1.0
        assert fresh["passed"] is True

        # A carrier for another tenant is refused at the door (the isolation
        # itself is the L3 two-tenant served-tests case), and the owner's
        # answer is unchanged.
        foreign = _client(FOREIGN_TENANT)
        try:
            with pytest.raises(Exception, match="AUTH_TENANT_MISMATCH"):
                foreign.jobs.submit(graph, {"QualityEvaluate": {"target": source}})
        finally:
            foreign.close()
        [again] = _evaluate(client, graph, "graph:Customer")
        assert (again["rows_checked"], again["rows_failed"]) == (2, 1)
    finally:
        client.tenants.delete(graph)
        client.close()
