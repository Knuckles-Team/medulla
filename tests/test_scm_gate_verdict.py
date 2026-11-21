"""MEDULLA-SCM-R008: the served ``RecordGateVerdict`` method.

A changeset lands on the default branch, a pass and a fail verdict are recorded for two
gates and read back as sealed ``GateVerdict`` nodes with their ``judges`` edge. An
identical
replay succeeds; a different verdict for the same gate refuses ``GATE_VERDICT_SEALED``;
a
digest that is not the changeset tree refuses ``GATE_INPUT_DIGEST_MISMATCH``.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid

import msgpack
import pytest
from conftest import TEST_AGENT_ID, request_context

from medulla.client import MedullaClient

MODE = 0o100644


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _listing_digest(entries: list[dict]) -> str:
    text = "".join(
        f"{e['path']}\t{e['blob_digest']}\t{e['mode']:o}\n"
        for e in sorted(entries, key=lambda e: e["path"])
    )
    return hashlib.sha256(text.encode()).hexdigest()


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "SetPathGrant",
            {
                "request": {
                    "repo": graph,
                    "principal": TEST_AGENT_ID,
                    "prefix": "",
                    "read": True,
                    "write": True,
                }
            },
        )
        data = f"gate {uuid.uuid4().hex[:8]}".encode()
        digest = await client.blob.store(data)
        receipt = await client._send(
            "SubmitChangeset",
            {
                "request": {
                    "repo": graph,
                    "branch": "main",
                    "base_head": None,
                    "message": "gate\n\nSpec: MEDULLA-SCM-R008",
                    "paths": [
                        {
                            "path": "a.txt",
                            "op": "add",
                            "blob": {"digest": digest, "size": len(data)},
                            "mode": MODE,
                            "renamed_from": None,
                        }
                    ],
                    "trailers": None,
                    "run_trace_id": None,
                }
            },
            idempotency_key=f"gate-{graph}",
        )
        changeset = receipt["changeset"]["id"]
        tree = await client._send(
            "ReadTree",
            {
                "request": {
                    "repo": graph,
                    "branch": "main",
                    "rev": receipt["branch_head"],
                    "prefix": "",
                    "depth": 0,
                }
            },
        )
        input_digest = _listing_digest(tree["entries"])
        evidence = hashlib.sha256(b"log").hexdigest()

        async def record(gate: str, verdict: str, digest_in: str = input_digest):
            return _body(
                await client._send(
                    "RecordGateVerdict",
                    {
                        "changeset": changeset,
                        "branch": "main",
                        "gate_id": gate,
                        "input_digest": digest_in,
                        "verdict": verdict,
                        "evidence_digest": evidence,
                        "duration_ms": 12,
                    },
                    graph,
                )
            )

        passed = await record("fmt", "pass")
        failed = await record("test", "fail")
        assert passed["node_id"] == f"gate_verdict:{changeset}:fmt"
        assert failed["node_id"] == f"gate_verdict:{changeset}:test"
        assert passed["replayed"] is False and passed["seal"] != failed["seal"]

        # Both read back from the graph, with the one judges edge each.
        for gate, verdict in (("fmt", "pass"), ("test", "fail")):
            node = _body(
                await client._send(
                    "GetNodeProperties",
                    {"node_id": f"gate_verdict:{changeset}:{gate}"},
                    graph,
                )
            )
            assert node["verdict"] == verdict and node["type"] == "GateVerdict"

        replay = await record("fmt", "pass")
        assert replay["replayed"] is True and replay["seal"] == passed["seal"]
        with pytest.raises(Exception, match="GATE_VERDICT_SEALED"):
            await record("fmt", "fail")
        with pytest.raises(Exception, match="GATE_INPUT_DIGEST_MISMATCH"):
            await record("lint", "pass", "0" * 64)
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R008
@pytest.mark.spec("MEDULLA-SCM-R008")
def test_verdict_records_are_sealed_and_read_back():
    asyncio.run(_scenario(f"scm-gate-{uuid.uuid4().hex[:8]}"))
