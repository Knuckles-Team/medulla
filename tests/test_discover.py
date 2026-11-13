"""``GraphOperationsClient.discover`` sends the one-round-trip Discover RPC
(CONCEPT:MEDULLA-KG.retrieval.one-round-trip-discovery) — hybrid keyword + semantic
discovery
that returns hydrated
node text.

The engine-side ranking (HNSW dense retrieval + keyword re-rank over
``name``/``description``/``type`` + text hydration) is covered by the Rust
dispatch test ``test_discover_blends_keyword_and_semantic``; this asserts the
Python client marshals the keyword set, embedding, and ``k`` into exactly one
``Discover`` call and returns the hydrated rows unchanged.
"""

from __future__ import annotations

from typing import Any

import pytest

from medulla.client import GraphOperationsClient, MedullaClient

# Fake-client unit tests only -- never needs the shared native engine (see
# conftest.py's session-scoped `start_medulla_server` fixture,
# which this marker exempts this module from triggering).
pytestmark = pytest.mark.no_engine


class _FakeClient(MedullaClient):
    def __init__(self) -> None:
        self.sent: list[tuple[str, dict[str, Any] | None]] = []

    async def _send(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        graph: str | None = None,
        *,
        idempotency_key: str | None = None,
    ) -> Any:
        self.sent.append((method, params))
        return {
            "hits": [
                {
                    "id": "deploy_runbook",
                    "name": "Deploy Runbook",
                    "description": "how to deploy a service",
                    "type": "doc",
                    "score": 0.87,
                }
            ],
            "verdict": {"gate_passed": True, "index_generation": 1},
            "plan": {"expansions": []},
        }


@pytest.mark.asyncio
async def test_discover_sends_one_rpc() -> None:
    fake = _FakeClient()
    gc = GraphOperationsClient(fake)
    out = await gc.discover(["deploy", "service"], [0.1, 0.2, 0.3], k=7)
    assert fake.sent == [
        (
            "Discover",
            {
                "keywords": ["deploy", "service"],
                "query_embedding": [0.1, 0.2, 0.3],
                "k": 7,
            },
        )
    ]
    hits = out["hits"]
    assert hits[0]["id"] == "deploy_runbook"
    assert hits[0]["name"] == "Deploy Runbook"
    assert hits[0]["type"] == "doc"
    assert "score" in hits[0]
    assert out["verdict"]["gate_passed"] is True


@pytest.mark.asyncio
async def test_discover_empty_embedding_keyword_only() -> None:
    """An empty embedding (embedder/vLLM unavailable) is a valid keyword-only call
    and defaults ``k`` to 5."""
    fake = _FakeClient()
    gc = GraphOperationsClient(fake)
    await gc.discover(["deploy"], [])
    assert fake.sent[0][0] == "Discover"
    assert fake.sent[0][1] is not None
    assert fake.sent[0][1]["query_embedding"] == []
    assert fake.sent[0][1]["k"] == 5


# spec: MEDULLA-FEDERATED-QUERY-R090.1
@pytest.mark.asyncio
async def test_discover_client_sends_planner_fields_and_reads_plan_vector() -> None:
    """The client carries the planner fields and the plan vector of the Rust test
    `discover_plan_round_trips` reads back unchanged."""
    fake = _FakeClient()
    gc = GraphOperationsClient(fake)
    await gc.discover(
        ["deploy"],
        [],
        k=3,
        query_text="how do I deploy",
        mode="deep",
        as_of=1_700_000_000,
        hops=2,
    )
    method, params = fake.sent[0]
    assert method == "Discover"
    assert params == {
        "keywords": ["deploy"],
        "query_embedding": [],
        "k": 3,
        "mode": "deep",
        "hops": 2,
        "query_text": "how do I deploy",
        "as_of": 1_700_000_000,
    }

    vector = {
        "hits": [
            {
                "id": "a",
                "name": "a",
                "description": "",
                "type": "doc",
                "score": 1.0,
                "arm_ranks": [{"arm": "expansion", "rank": 1}],
                "recency_boost": 1.25,
                "hop_path": ["s", "a"],
            }
        ],
        "plan": {
            "expansions": [
                {
                    "digest": "0" * 64,
                    "kind": "sub_query",
                    "provider_id": "m",
                    "provider_revision": "r",
                }
            ],
            "degraded": {"state": "provider_failed", "message": "down"},
        },
    }

    class _Vector(_FakeClient):
        async def _send(self, *args: Any, **kwargs: Any) -> Any:
            return vector

    out = await GraphOperationsClient(_Vector()).discover(["a"], [], k=1)
    assert out["plan"]["expansions"][0]["digest"] == "0" * 64
    assert out["hits"][0]["hop_path"] == ["s", "a"]
