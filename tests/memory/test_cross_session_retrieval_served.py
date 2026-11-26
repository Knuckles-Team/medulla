"""MEDULLA-DECISION-ENGINE-R155.6 served cross-session recall of sealed episodes.

Three sessions are sealed through ``MemoryRemember`` (the last record of each
carries the episode spec) and a fourth ends in the future. A later recall
through ``MemoryRecall`` with ``episode_scope`` lists the earlier episodes in
time order, hides the future one, and a sealed episode refuses a second write
with a different body.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient

SESSIONS = (
    ("a", [1_000, 1_100]),
    ("b", [2_000, 2_100, 2_200]),
    ("c", [3_000]),
)
FUTURE_MS = 4_000_000_000_000


def _record(
    prefix: str, memory_id: str, session: str, timestamp_ms: int
) -> dict[str, Any]:
    return {
        "memory_id": f"{prefix}:{memory_id}",
        "tenant": TEST_TENANT,
        "kind": "episode",
        "tier": "episodic",
        "text": f"note of session {session} at {timestamp_ms}",
        "session_id": f"{prefix}:{session}",
        "timestamp_ms": timestamp_ms,
        "importance": 1.0,
        "token_estimate": 10,
    }


async def _seal(
    decision: dc.DecisionClient, prefix: str, session: str, stamps: list[int]
) -> dict[str, Any]:
    ids = [f"{session}{index}" for index in range(len(stamps))]
    for memory_id, at in list(zip(ids, stamps, strict=True))[:-1]:
        await decision.memory_remember(
            _record(prefix, memory_id, session, at), [1.0, 0.0]
        )
    return await decision.memory_remember(
        _record(prefix, ids[-1], session, stamps[-1]),
        [1.0, 0.0],
        episode={
            "episode_id": f"{prefix}:ep:{session}",
            "session_id": f"{prefix}:{session}",
            "preceding": [f"{prefix}:{memory_id}" for memory_id in ids[:-1]],
        },
    )


async def _journey(prefix: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        sealed = {}
        # Sealed out of order, recalled in time order.
        for session, stamps in (SESSIONS[1], SESSIONS[0], SESSIONS[2]):
            sealed[session] = await _seal(decision, prefix, session, stamps)
        await _seal(decision, prefix, "future", [FUTURE_MS])

        recalled = await decision.memory_recall(
            {
                "keywords": ["note"],
                "budget": 10_000,
                "episode_scope": "episodes",
                "as_of_ms": 5_000,
            }
        )
        mine = [
            view
            for view in recalled["episodes"]
            if view["episode_id"].startswith(f"{prefix}:")
        ]
        assert [view["episode_id"] for view in mine] == [
            f"{prefix}:ep:a",
            f"{prefix}:ep:b",
            f"{prefix}:ep:c",
        ]
        assert [view["record_count"] for view in mine] == [2, 3, 1]
        assert recalled["as_of_ms"] == 5_000
        assert recalled["episode_scope"] == "episodes"
        # The digest the remember reported is the one recall reports.
        assert mine[0]["digest"] == sealed["a"]["episode"]["digest"]
        held = {hit["record"]["memory_id"] for hit in recalled["hits"]}
        assert f"{prefix}:a0" in held and f"{prefix}:future0" not in held

        # A sealed episode is immutable.
        with pytest.raises(EngineResponseError) as refused:
            await decision.memory_remember(
                _record(prefix, "a2", "a", 1_200),
                [1.0, 0.0],
                episode={
                    "episode_id": f"{prefix}:ep:a",
                    "session_id": f"{prefix}:a",
                    "preceding": [f"{prefix}:a0", f"{prefix}:a1"],
                },
            )
        assert "EPISODE_IMMUTABLE" in str(refused.value)
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R155.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R155.6")
def test_later_session_recalls_earlier_episode(clean_graph):
    asyncio.run(_journey(f"xs{uuid.uuid4().hex[:8]}"))
