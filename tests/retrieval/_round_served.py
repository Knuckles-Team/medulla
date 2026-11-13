"""Shared helpers of the served retrieval-round tests (MEDULLA-FEDERATED-QUERY-R081.11).

Each helper talks to the real server through the Python client: nothing is
stubbed. `hashed` reproduces the server's deterministic `hash` text embedder
(SipHash-1-3 with zero keys over the component index, the text and a 0xff
terminator, eight components, L2-normalised), so stored vectors share a space
with the query vectors the server embeds.
"""

from __future__ import annotations

import os
import struct
from typing import Any

import msgpack
from conftest import request_context

_MASK = (1 << 64) - 1


def client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def put(
    cl: Any,
    graph: str,
    node_id: str,
    props: dict[str, Any],
    embedding: list[float] | None = None,
) -> None:
    cl._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )
    if embedding is not None:
        cl._send("AddEmbedding", {"node_id": node_id, "embedding": embedding}, graph)


def run_round(cl: Any, graph: str, request: dict[str, Any]) -> dict[str, Any]:
    result = cl._send("RetrievalRound", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def request(plan_ops: list[dict[str, Any]], **parts: Any) -> dict[str, Any]:
    body: dict[str, Any] = {
        "plan": {"ops": plan_ops},
        "corpus_revision": "corpus-1",
        "model_revision": "model-1",
        "sufficiency": {
            "round": 1,
            "round_limit": 4,
            "k": 4,
            "fallback_mode": "extractive",
            "fallback": "lenient",
        },
    }
    body.update(parts)
    return body


def _rotl(x: int, b: int) -> int:
    return ((x << b) | (x >> (64 - b))) & _MASK


def _siphash13(data: bytes) -> int:
    v0, v1 = 0x736F6D6570736575, 0x646F72616E646F6D
    v2, v3 = 0x6C7967656E657261, 0x7465646279746573

    def rounds(count: int) -> None:
        nonlocal v0, v1, v2, v3
        for _ in range(count):
            v0 = (v0 + v1) & _MASK
            v1 = _rotl(v1, 13) ^ v0
            v0 = _rotl(v0, 32)
            v2 = (v2 + v3) & _MASK
            v3 = _rotl(v3, 16) ^ v2
            v0 = (v0 + v3) & _MASK
            v3 = _rotl(v3, 21) ^ v0
            v2 = (v2 + v1) & _MASK
            v1 = _rotl(v1, 17) ^ v2
            v2 = _rotl(v2, 32)

    whole = len(data) - len(data) % 8
    for i in range(0, whole, 8):
        m = struct.unpack("<Q", data[i : i + 8])[0]
        v3 ^= m
        rounds(1)
        v0 ^= m
    tail = data[whole:] + b"\x00" * (7 - len(data) % 8)
    last = struct.unpack("<Q", tail + bytes([len(data) & 0xFF]))[0]
    v3 ^= last
    rounds(1)
    v0 ^= last
    v2 ^= 0xFF
    rounds(3)
    return v0 ^ v1 ^ v2 ^ v3


def hashed(text: str, dim: int = 8) -> list[float]:
    components = [
        _siphash13(struct.pack("<Q", d) + text.encode() + b"\xff") / _MASK * 2.0 - 1.0
        for d in range(dim)
    ]
    norm = sum(c * c for c in components) ** 0.5
    return [c / norm for c in components] if norm > 0 else components
