"""MEDULLA-DECISION-ENGINE-R154.6 served proof that recall needs no retuning.

The journey writes records through ``MemoryRemember`` at three corpus sizes
(each stage adds filler facts) and recalls the same vector and keyword queries
through ``MemoryRecall`` after each. The five needle facts rank first by cosine
and by keyword, so the result set is equal at every size.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

NEEDLES = 5
SIZES = (40, 80, 160)


def _record(tenant: str, memory_id: str, text: str) -> dict[str, Any]:
    return {
        "memory_id": memory_id,
        "tenant": tenant,
        "kind": "fact",
        "tier": "semantic",
        "text": text,
        "timestamp_ms": 10,
        "importance": 1.0,
        "token_estimate": 10,
    }


def _embedding(index: int) -> list[float]:
    return [1.0, 0.1 * index] if index < NEEDLES else [0.0, 1.0]


async def _journey(prefix: str) -> list[tuple[list[str], list[str]]]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        found: list[tuple[list[str], list[str]]] = []
        written = 0
        for size in SIZES:
            for index in range(written, size):
                text = (
                    f"needle fact {index}"
                    if index < NEEDLES
                    else f"filler fact {index}"
                )
                await decision.memory_remember(
                    _record(TEST_TENANT, f"{prefix}:{index:05}", text),
                    _embedding(index),
                    idempotency_key=f"{prefix}:{index}",
                )
            written = size
            by_vector = await decision.memory_recall(
                {"query_embedding": [1.0, 0.0], "budget": 1000}
            )
            by_keyword = await decision.memory_recall(
                {"keywords": ["needle"], "budget": 1000}
            )
            found.append(
                (
                    [hit["record"]["memory_id"] for hit in by_vector["hits"]],
                    [hit["record"]["memory_id"] for hit in by_keyword["hits"]],
                )
            )
        return found
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R154.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R154.6")
def test_recall_stable_as_corpus_grows(clean_graph):
    prefix = f"scale{uuid.uuid4().hex[:8]}"
    found = asyncio.run(_journey(prefix))
    for by_vector, by_keyword in found:
        assert len(by_vector) == NEEDLES, by_vector
        assert len(by_keyword) == NEEDLES, by_keyword
    assert found[0] == found[1] == found[2]
