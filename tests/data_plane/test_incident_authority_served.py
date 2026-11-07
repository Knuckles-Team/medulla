"""MEDULLA-UNIFIED-DATA-PLANE-R760.3: the five served incident methods.

Every method answers under the verified tenant of the request carrier; a
tenant named in the request body is not part of the wire shape and is
refused, never used.
"""

from __future__ import annotations

import os
import time

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _reading(entity: str, at_ms: int) -> dict:
    return {
        "entity_ref": entity,
        "signal": "latency",
        "observed": 300.0,
        "baseline": 100.0,
        "at_ms": at_ms,
    }


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R760.3")
@pytest.mark.asyncio
async def test_five_methods_served_under_caller_tenant(start_medulla_server) -> None:
    from medulla.client import MedullaClient

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        recorded = _body(
            await client._send(
                "IncidentRecordAnomaly",
                {"anomaly": _reading("svc:r760-3", 10), "window_ms": 60_000},
                GRAPH,
            )
        )
        [incident_id] = recorded["incident_ids"]
        assert recorded["nodes_written"] >= 2

        joined = _body(
            await client._send(
                "IncidentCorrelate",
                {"anomalies": [_reading("svc:r760-3", 20)], "window_ms": 60_000},
                GRAPH,
            )
        )
        assert joined["incident_ids"] == [incident_id], "the open incident is reused"

        record = _body(
            await client._send("IncidentEvidence", {"incident_id": incident_id}, GRAPH)
        )
        assert record["tenant"] == TEST_TENANT
        assert len(record["signature_digest"]) == 64
        assert len(record["evidence"]) == 2

        claims = _body(
            await client._send(
                "IncidentRelatedClaims", {"incident_id": incident_id}, GRAPH
            )
        )
        assert claims == []

        with pytest.raises(Exception, match="INCIDENT_STATUS_SKIP"):
            await client._send(
                "IncidentSetStatus",
                {"incident_id": incident_id, "status": "resolved"},
                GRAPH,
            )
        moved = _body(
            await client._send(
                "IncidentSetStatus",
                {"incident_id": incident_id, "status": "acknowledged"},
                GRAPH,
            )
        )
        assert moved["incident_ids"] == [incident_id]

        with pytest.raises(Exception, match="ANOMALY_RESOURCE_REQUIRED"):
            await client._send(
                "IncidentCorrelate",
                {"anomalies": [_reading("", 30)], "window_ms": 60_000},
                GRAPH,
            )
        # A body tenant is not part of the wire shape: it never selects a scope.
        with pytest.raises(Exception, match="INVALID_ARGUMENT|unknown field"):
            await client._send(
                "IncidentEvidence",
                {"incident_id": incident_id, "tenant": "tenant:other"},
                GRAPH,
            )
    finally:
        await client.close()

    other = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context={**request_context(), "tenant": "tenant:other"},
    )
    try:
        with pytest.raises(Exception, match="INCIDENT_NOT_FOUND|AUTH_TENANT_MISMATCH"):
            await other._send("IncidentEvidence", {"incident_id": incident_id}, GRAPH)
    finally:
        await other.close()


ANOMALIES = 10_000
ENTITIES = 100
# MEDULLA-UNIFIED-DATA-PLANE-R760.6 states this budget for the release profile.
P99_BUDGET_MS = 25.0


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R760.6")
@pytest.mark.asyncio
async def test_anomalies_correlate_into_one_incident(start_medulla_server) -> None:
    from medulla.client import MedullaClient

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        profile = (await client.health())["build_profile"]
        prefix = f"svc:r760-6-{os.getpid()}-"
        readings = [
            _reading(f"{prefix}{i % ENTITIES}", i // ENTITIES) for i in range(ANOMALIES)
        ]
        request = {"anomalies": readings, "window_ms": 60_000}
        started = time.perf_counter()
        joined = _body(await client._send("IncidentCorrelate", request, GRAPH))
        elapsed_ms = (time.perf_counter() - started) * 1_000.0
        assert len(joined["incident_ids"]) == ENTITIES, "one incident per signature"
        # One write: each incident node and each reading's evidence node.
        assert joined["nodes_written"] == ENTITIES + ANOMALIES
        if profile == "release":
            assert elapsed_ms < P99_BUDGET_MS, f"correlate took {elapsed_ms:.1f} ms"

        # The same readings again join the same incidents and write nothing:
        # evidence an incident already holds is never rewritten.
        replay = _body(await client._send("IncidentCorrelate", request, GRAPH))
        assert sorted(replay["incident_ids"]) == sorted(joined["incident_ids"])
        assert replay["nodes_written"] == 0
    finally:
        await client.close()
