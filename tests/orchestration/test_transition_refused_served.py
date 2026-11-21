"""MEDULLA-ORCH-VOCAB-R811.9 served proof: an illegal state-machine transition
answers the four fields of the typed refusal record to the Python client."""

from __future__ import annotations

import os

import pytest
from conftest import request_context

GRAPH = "__commons__"


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R811.9")
@pytest.mark.asyncio
async def test_illegal_transition_returns_all_four_fields(start_medulla_server) -> None:
    from medulla.client import MedullaClient, TransitionRefusedError

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        recorded = await client._send(
            "IncidentRecordAnomaly",
            {
                "anomaly": {
                    "entity_ref": "svc:r811-9",
                    "signal": "latency",
                    "observed": 300.0,
                    "baseline": 100.0,
                    "at_ms": 10,
                },
                "window_ms": 60_000,
            },
            GRAPH,
        )
        import msgpack

        body = (
            msgpack.unpackb(recorded, raw=False)
            if isinstance(recorded, (bytes, bytearray))
            else recorded
        )
        [incident_id] = body["incident_ids"]
        with pytest.raises(TransitionRefusedError) as refused:
            await client._send(
                "IncidentSetStatus",
                {"incident_id": incident_id, "status": "resolved"},
                GRAPH,
            )
    finally:
        await client.close()
    assert refused.value.code == "TRANSITION_REFUSED"
    assert refused.value.machine == "IncidentRecordMachine"
    assert refused.value.from_state == "open"
    assert refused.value.event == "resolve"
    assert refused.value.reason == "illegal_transition"
