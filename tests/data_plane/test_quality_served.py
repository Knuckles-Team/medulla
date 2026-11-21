"""MEDULLA-UNIFIED-DATA-PLANE-R762.3: approved quality rules scored by a job.

`QualityRulePut` stores an approved rule under the verified tenant. Each
`QualityEvaluate` job scores it; a streak of failures opens one incident that
`IncidentEvidence` reads back under the same tenant.
"""

from __future__ import annotations

import hashlib
import json
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


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.5")
def test_score_evidence_reads_back(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.5: digest, row counts and checker."""
    client = _client()
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    table = f"orders_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client.query.sql(f"CREATE TABLE {table} (id int primary key, status text)")
        client.query.sql(
            f"INSERT INTO {table} VALUES (1, 'open'), (2, 'open'), (3, NULL)"
        )
        rule = {
            "id": "status-evidence",
            "target": table,
            "dimension": "completeness",
            "expression": "status IS NOT NULL",
            "threshold": 0.5,
            "severity": "warning",
        }
        _decode(client._send("QualityRulePut", {"rule": rule}, graph))
        [score] = _evaluate(client, graph, table)
        statement = (
            "SELECT COUNT(*) AS rows_checked, "
            f"COUNT(*) FILTER (WHERE {rule['expression']}) AS rows_passed FROM {table}"
        )
        digest = hashlib.sha256(statement.encode()).hexdigest()
        assert score["query_digest"] == digest
        assert len(score["query_digest"]) == 64
        assert score["query_digest"] == score["query_digest"].lower()
        assert (score["rows_checked"], score["rows_failed"]) == (3, 1)
        assert score["rows_failed"] <= score["rows_checked"]
        assert score["checker"] == "native"
        assert score["value"] == pytest.approx(2 / 3)
        assert score["passed"] is True
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.4")
def test_quality_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.4: refusals write nothing and a
    proposed rule is counted, never evaluated."""
    client = _client()
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    table = f"orders_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client.query.sql(f"CREATE TABLE {table} (id int primary key, status text)")
        client.query.sql(f"INSERT INTO {table} VALUES (1, NULL), (2, 'open')")
        rule = {
            "id": "status-refusals",
            "target": table,
            "dimension": "completeness",
            "expression": "status IS NOT NULL",
            "threshold": 0.9,
            "severity": "critical",
        }
        for invalid, code in (
            ({**rule, "id": " "}, "QUALITY_RULE_INVALID"),
            ({**rule, "target": " "}, "QUALITY_RULE_INVALID"),
            ({**rule, "expression": "1; DROP TABLE x"}, "QUALITY_RULE_INVALID"),
            ({**rule, "threshold": 1.5}, "QUALITY_THRESHOLD_INVALID"),
            ({**rule, "threshold": -0.1}, "QUALITY_THRESHOLD_INVALID"),
        ):
            with pytest.raises(Exception, match=code):
                client._send("QualityRulePut", {"rule": invalid}, graph)

        # Every refusal wrote no node: the target has no rule to score.
        [none_stored] = _evaluate(client, graph, table)
        assert none_stored["rule"] == ""
        assert none_stored["skipped_proposed"] == 0

        # A rule in the proposed state is skipped by evaluation and counted.
        client._send(
            "AddNode",
            {
                "node_id": f"quality_rule:proposed:{uuid.uuid4().hex[:8]}",
                "properties_msgpack": msgpack.packb(
                    {
                        "type": "QualityRule",
                        "rule": json.dumps(rule),
                        "state": {"state": "proposed"},
                        "tenant_id": TEST_TENANT,
                    }
                ),
            },
            graph,
        )
        [skipped] = _evaluate(client, graph, table)
        assert skipped["rule"] == ""
        assert skipped["value"] is None
        assert skipped["skipped_proposed"] == 1
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


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.6")
def test_hundred_rules_over_a_native_dataset(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.6: 100 rules over 100,000 native rows."""
    client = _client()
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    table = f"q100_{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        client.query.sql(f"CREATE TABLE {table} (id int primary key, v int)")
        for batch in range(100):
            rows = ", ".join(
                f"({batch * 1000 + n}, {(batch * 1000 + n) % 100})" for n in range(1000)
            )
            client.query.sql(f"INSERT INTO {table} VALUES {rows}")
        for n in range(100):
            rule = {
                "id": f"r{n:03d}",
                "target": table,
                "dimension": "validity",
                "expression": f"v >= {n}",
                "threshold": 0.5,
                "severity": "warning",
            }
            receipt = _decode(client._send("QualityRulePut", {"rule": rule}, graph))
            assert receipt["rule_id"] == rule["id"]

        rows = _evaluate(client, graph, table)
        assert len(rows) == 100
        assert sorted(row["rule"] for row in rows) == [f"r{n:03d}" for n in range(100)]
        assert all(isinstance(row["passed"], bool) for row in rows)
        # `v >= N` holds for at least half the rows only when N <= 50.
        assert sum(1 for row in rows if row["passed"]) == 51
    finally:
        client.tenants.delete(graph)
        client.close()


def _outside_declaration(connector: str) -> Any:
    """A `QualityScore` mapping: `checker`, `rule` and `score` name the score's properties."""
    from medulla import ConnectorPackEntryContent
    from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

    body = {
        "connector": connector,
        "schema_mappings": {
            "score": {
                "fields": {
                    "id": "identifier",
                    "checker": "checker",
                    "rule": "rule",
                    "score": "value",
                },
                "ontology_class": "QualityScore",
            }
        },
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        output_schema=b'{"type":"object"}',
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


def _outside_batch(connector: str, page: int, previous: int | None, payload: dict) -> dict:
    stream = "scores"
    checkpoint = lambda n: {  # noqa: E731
        "stream": stream,
        "position": {"page": n},
        "watermark": "2026-09-20T00:00:00Z",
    }
    return {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": True,
        "records": [
            {
                "stream": stream,
                "record_id": f"score-{page}",
                "mapping_reference": f"connector:{connector}#mappings/score",
                "payload": payload,
                "updated_at": "2026-09-20T00:00:00Z",
                "provenance": {
                    "connector": connector,
                    "adapter_kind": "mcp",
                    "server": "demo-server",
                    "tool": "list_scores",
                    "tool_schema_sha256": "07" * 32,
                    "source_uri": f"demo://scores/{page}",
                },
            }
        ],
        "relationships": [],
        "withdrawals": [],
        "provider_checkpoint": checkpoint(page),
        "expected_previous_checkpoint": None if previous is None else checkpoint(previous),
    }


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R762.8")
@pytest.mark.asyncio
async def test_outside_scores_open_incident(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R762.8: three failing outside scores open one incident."""
    import sys
    from pathlib import Path

    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
    from ontology_pack_support import connect, import_entries

    from medulla.client import MedullaClient

    connector = f"outside-{uuid.uuid4().hex[:10]}"
    graph = f"quality-{uuid.uuid4().hex[:8]}"
    admin = await connect()
    try:
        await import_entries(admin, connector, [_outside_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        rule = {
            "id": "email-valid",
            "target": "contacts",
            "dimension": "validity",
            "expression": "email LIKE '%@%'",
            "threshold": 0.9,
            "severity": "critical",
        }
        _decode(await client._send("QualityRulePut", {"rule": rule}))

        async def submit(page: int, previous: int | None, **payload: Any) -> Any:
            batch = _outside_batch(
                connector, page, previous, {"id": page, "checker": "outside-dq", **payload}
            )
            return await client._send(
                "SourceIngest",
                {"request": batch},
                idempotency_key=f"outside-{page}-{uuid.uuid4().hex}",
            )

        # A rule the tenant never approved, and a value outside 0 to 1, refuse.
        with pytest.raises(Exception, match="QUALITY_RULE_UNKNOWN"):
            await submit(2, None, rule="no-such-rule", score=0.1)
        with pytest.raises(Exception, match="QUALITY_THRESHOLD_INVALID"):
            await submit(2, None, rule="email-valid", score=1.5)

        await submit(2, None, rule="email-valid", score=0.1)
        await submit(3, 2, rule="email-valid", score=0.2)
        assert not await client.nodes.list_by_label("Incident", 0)
        await submit(4, 3, rule="email-valid", score=0.3)

        incidents = await client.nodes.list_by_label("Incident", 0)
        assert len(incidents) == 1
        incident_id = incidents[0][0]
        record = _decode(await client._send("IncidentEvidence", {"incident_id": incident_id}))
        assert record["tenant"] == TEST_TENANT

        scores = [
            props
            for node_id, _ in await client.nodes.list_by_label("QualityScore", 0)
            if (props := await client.nodes.properties(node_id)) and props.get("rule")
        ]
        assert sorted(score["value"] for score in scores) == [0.1, 0.2, 0.3]
        assert {score["checker"] for score in scores} == {"outside-dq"}
        assert not any(score["passed"] for score in scores)
    finally:
        await client.tenants.delete(graph)
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R805.5")
def test_violation_evidence_reads_back(start_medulla_server) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R805.5: a broken quality promise leaves a violation node."""
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
        _decode(client._send("QualityRulePut", {"rule": rule}, graph))
        context = request_context()
        product = {
            "id": "orders-product",
            "owner": context["agent_id"],
            "version": "1.0.0",
            "datasets": [table],
            "terms": [],
            "ports": [{"name": "sql", "kind": "query"}],
        }
        contract = {
            "product": "orders-product",
            "schema_digest": "sha256:" + "a" * 64,
            "quality_rules": ["status-complete"],
            "freshness_s": 3600,
        }
        _decode(
            client._send(
                "DataProductRegister",
                {"tenant": TEST_TENANT, "product": product, "contract": contract},
                graph,
            )
        )
        _evaluate(client, graph, table)
        nodes = _decode(
            client._send(
                "GetNodesByLabel", {"label": "ContractViolation", "limit": 10}, graph
            )
        )
        violations = [_decode(properties) for _, properties in nodes]
        [broken] = [v for v in violations if v["promise"] == "quality"]
        assert broken["product"] == "orders-product"
        assert broken["rule"] == "status-complete"
        assert broken["evidence_class"] == "observation"
        assert broken["observed"] == "0"
        assert broken["at_ms"] > 0
    finally:
        client.tenants.delete(graph)
        client.close()
