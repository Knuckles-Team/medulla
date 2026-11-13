"""Served document-chunk encoding (MEDULLA-FEDERATED-QUERY-R039.2).

The session fixture in conftest starts and authenticates the native server. The tests
drive the generated ``EncodeDocument`` and ``EncodedDocumentSearch`` requests. Each test
pins its own policy through a unique tokenizer identity, so its policy digest names a
vector generation no other test touches and generations count from one.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import struct
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

MODEL = "builtin:test-hashed-token/1"
DIM = 8
POISON_NON_FINITE = 2**32 - 1
POISON_SHORT = 2**32 - 2


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _policy(
    size: int = 16, overlap: int = 4, mode: str = "local_reset", **over: object
) -> dict:
    policy: dict = {
        "policy_revision": 1,
        "tokenizer": f"token-ids-{uuid.uuid4().hex}",
        "chunk_size": size,
        "overlap": overlap,
        "position_mode": mode,
        "aggregation": "mean_chunks",
        "model_revision": MODEL,
    }
    policy.update(over)
    return policy


def _digest(policy: dict) -> str:
    # The server digests the policy record in field order, compactly.
    return hashlib.sha256(
        json.dumps(policy, separators=(",", ":")).encode()
    ).hexdigest()


async def _encode(
    client: MedullaClient, doc: str, tokens: list[int], policy: dict
) -> dict:
    request = {"document_id": doc, "token_ids": tokens, "policy": policy}
    return (await query.send_encode_document(client, request)).payload


async def _search(
    client: MedullaClient, vector: list[float], digest: str, k: int = 5
) -> dict:
    request = {"request": {"query_embedding": vector, "k": k, "policy_digest": digest}}
    return (await query.send_encoded_document_search(client, request)).payload


def _oracle_chunk(window: list[int], base: int) -> list[float]:
    """The bound test encoder, re-derived independently of the server."""
    vector = [0.0] * DIM
    for offset, token in enumerate(window):
        bucket = ((token * 2654435761) % 2**32 >> 7) % DIM
        vector[bucket] += 1.0 + ((base + offset) % 5) * 0.25
    return vector


def _oracle_document(
    tokens: list[int], size: int, overlap: int, absolute: bool
) -> list[float]:
    chunks = []
    start = 0
    while start < len(tokens):
        end = min(start + size, len(tokens))
        chunks.append(_oracle_chunk(tokens[start:end], start if absolute else 0))
        if end == len(tokens):
            break
        start += size - overlap
    mean = [sum(c[i] for c in chunks) / len(chunks) for i in range(DIM)]
    norm = math.sqrt(sum(x * x for x in mean))
    return [x / norm for x in mean]


def _f32(values: list[float]) -> bytes:
    return struct.pack(f"<{len(values)}f", *values)


TOKENS = [(i * 7 + 3) % 500 for i in range(40)]


# spec: MEDULLA-FEDERATED-QUERY-R039.2.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.2.5")
@pytest.mark.asyncio
async def test_ingest_and_query_share_policy_generation() -> None:
    client = await _connect()
    try:
        policy = _policy()
        digest = _digest(policy)
        first = await _encode(client, "doc-a", TOKENS, policy)
        second = await _encode(client, "doc-b", TOKENS[::-1], policy)
        assert first["receipt"]["policy_digest"] == digest
        assert second["receipt"]["policy_digest"] == digest
        assert (
            first["receipt"]["index_generation"],
            second["receipt"]["index_generation"],
        ) == (1, 2)

        found = await _search(client, first["vector"], digest)
        # Query and ingestion read one policy digest and one vector generation.
        assert found["policy_digest"] == digest
        assert found["index_generation"] == second["receipt"]["index_generation"]
        assert found["hits"][0]["document_id"] == "doc-a"
        assert found["hits"][0]["score"] == pytest.approx(1.0, abs=1e-5)
        assert {h["receipt"]["policy_digest"] for h in found["hits"]} == {digest}
        assert [h["document_id"] for h in found["hits"]] == ["doc-a", "doc-b"]

        # A policy digest with no generation under it is stale and ranks nothing.
        with pytest.raises(RuntimeError, match="ENCODE_POLICY_STALE"):
            await _search(client, first["vector"], _digest(_policy()))
        # A model revision other than the bound one never publishes.
        with pytest.raises(RuntimeError, match="ENCODE_POLICY_STALE"):
            await _encode(client, "doc-c", TOKENS, _policy(model_revision="other/9"))
        # Another policy has its own generation counter and never mixes into this one.
        other = _policy(mode="absolute_offset")
        absolute = await _encode(client, "doc-a", TOKENS, other)
        assert absolute["receipt"]["index_generation"] == 1
        assert (await _search(client, first["vector"], digest))["index_generation"] == 2
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R039.2.6
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.2.6")
@pytest.mark.asyncio
async def test_failed_chunk_keeps_prior_generation() -> None:
    client = await _connect()
    try:
        policy = _policy()
        digest = _digest(policy)
        good = await _encode(client, "doc-ok", TOKENS, policy)

        negatives = [
            (
                "doc-nan",
                TOKENS[:20] + [POISON_NON_FINITE] + TOKENS[20:],
                policy,
                "ENCODE_NON_FINITE",
            ),
            (
                "doc-dim",
                TOKENS[:20] + [POISON_SHORT] + TOKENS[20:],
                policy,
                "ENCODE_DIM_MISMATCH",
            ),
            ("doc-empty", [], policy, "NO_EMBEDDING"),
            (
                "doc-overlap",
                TOKENS,
                _policy(size=16, overlap=16),
                "ENCODE_OVERLAP_INVALID",
            ),
            ("doc-stale", TOKENS, _policy(policy_revision=9), "ENCODE_POLICY_STALE"),
            (
                "doc-budget",
                [t % 500 for t in range(16 * 4097)],
                _policy(size=16, overlap=0),
                "ENCODE_BUDGET_EXHAUSTED",
            ),
        ]
        for doc, tokens, bad_policy, code in negatives:
            with pytest.raises(RuntimeError, match=code):
                await _encode(client, doc, tokens, bad_policy)

        # Every refusal wrote nothing: the prior generation still answers, alone.
        found = await _search(client, good["vector"], digest)
        assert found["index_generation"] == good["receipt"]["index_generation"] == 1
        assert [h["document_id"] for h in found["hits"]] == ["doc-ok"]
        # A failed re-encode of an existing document leaves its old vector in place.
        with pytest.raises(RuntimeError, match="ENCODE_NON_FINITE"):
            await _encode(client, "doc-ok", [POISON_NON_FINITE] * 3, policy)
        again = await _search(client, good["vector"], digest)
        assert again["hits"][0]["receipt"] == good["receipt"]
    finally:
        await client.close()


# spec: MEDULLA-FEDERATED-QUERY-R039.2.7
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.2.7")
@pytest.mark.asyncio
async def test_receipt_accounts_every_chunk() -> None:
    client = await _connect()
    try:
        for absolute in (False, True):
            policy = _policy(mode="absolute_offset" if absolute else "local_reset")
            stored = await _encode(client, "doc-r", TOKENS, policy)
            receipt = stored["receipt"]
            # Every chunk and the one stored vector appear in the receipt.
            assert receipt["token_ranges"] == [[0, 16], [12, 28], [24, 40]]
            assert receipt["chunk_model_revisions"] == [MODEL] * 3
            assert receipt["position_mode"] == (
                "absolute_offset" if absolute else "local_reset"
            )
            assert receipt["policy_digest"] == _digest(policy)
            assert (
                receipt["document_digest"]
                == hashlib.sha256(struct.pack(f"<{len(TOKENS)}I", *TOKENS)).hexdigest()
            )
            assert (
                receipt["aggregate_digest"]
                == hashlib.sha256(_f32(stored["vector"])).hexdigest()
            )
            assert receipt["index_generation"] == 1
            assert receipt["measured_cost_ms"] >= 0
            for field in ("document_digest", "aggregate_digest", "policy_digest"):
                assert (
                    len(receipt[field]) == 64
                    and receipt[field] == receipt[field].lower()
                )

            expected = _oracle_document(TOKENS, 16, 4, absolute)
            assert stored["vector"] == pytest.approx(expected, abs=1e-6)

            # The receipt is stored with the vector: a search returns the same record.
            found = await _search(
                client, stored["vector"], receipt["policy_digest"], k=1
            )
            assert found["hits"][0]["receipt"] == receipt
        # Local-reset and absolute-offset positions encode differently.
        assert _oracle_document(TOKENS, 16, 4, False) != pytest.approx(
            _oracle_document(TOKENS, 16, 4, True), abs=1e-6
        )
    finally:
        await client.close()
