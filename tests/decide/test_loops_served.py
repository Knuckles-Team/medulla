"""MEDULLA-DECISION-ENGINE-R653.3 served loops: a loop is claimed, a gap is filed
and a stage beacon is recorded through the ``loops`` client method."""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient


async def _journey(loop_id: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        claimed = await decision.loops(
            {
                "op": "claim",
                "loop_id": loop_id,
                "objective_digest": "sha256:objective",
                "priority": 3,
            }
        )
        epoch = claimed["instance"]["lease"]["epoch"]
        filed = await decision.loops(
            {
                "op": "file_gap",
                "loop_id": loop_id,
                "epoch": epoch,
                "gap_id": "g1",
                "severity_ppm": 800_000,
                "evidence": ["run:g1"],
            }
        )
        beaconed = await decision.loops(
            {"op": "beacon", "loop_id": loop_id, "epoch": epoch, "stage": "learn"}
        )
        return {"claimed": claimed, "filed": filed, "beaconed": beaconed}
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R653.3
# spec: MEDULLA-DECISION-ENGINE-R653.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R653.3")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R653.6")
def test_loop_claim_served(clean_graph):
    outcome = asyncio.run(_journey(f"loop-{uuid.uuid4().hex[:12]}"))

    claimed = outcome["claimed"]["instance"]
    assert claimed["state"] == "running"
    assert claimed["priority"] == 3
    assert claimed["lease"]["epoch"] == 1

    (gap,) = outcome["filed"]["gaps"]
    assert gap["gap_id"] == "g1"
    assert gap["state"] == "open"

    (beacon,) = outcome["beaconed"]["beacons"]
    assert beacon["stage"] == "learn"
