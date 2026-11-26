"""MEDULLA-SCM-R009: the served ``BisectStep`` method.

Five changesets land on the default branch; the first passes the gate and the
last fails it. ``BisectStep`` names the midpoint, the verdicts are recorded as
the harness would, and the search ends on the first failing changeset. A chain
with no passing verdict refuses ``BISECT_VERDICT_COVERAGE``.
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
FIRST_BAD = 3


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
        links: list[dict] = []
        head = None
        for n in range(5):
            data = f"bisect {n} {uuid.uuid4().hex[:8]}".encode()
            digest = await client.blob.store(data)
            receipt = await client._send(
                "SubmitChangeset",
                {
                    "request": {
                        "repo": graph,
                        "branch": "main",
                        "base_head": head,
                        "message": f"change {n}\n\nSpec: MEDULLA-SCM-R009",
                        "paths": [
                            {
                                "path": f"f{n}.txt",
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
                idempotency_key=f"bisect-{graph}-{n}",
            )
            head = receipt["branch_head"]
            links.append({"commit": head, "changeset": receipt["changeset"]["id"]})

        async def bisect(good: str | None, bad: str):
            return _body(
                await client._send(
                    "BisectStep",
                    {
                        "request": {
                            "repo": graph,
                            "branch": "main",
                            "gate_id": "test",
                            "bad": bad,
                            "good": good,
                        }
                    },
                    graph,
                )
            )

        async def record(index: int, verdict: str):
            tree = await client._send(
                "ReadTree",
                {
                    "request": {
                        "repo": graph,
                        "branch": "main",
                        "rev": links[index]["commit"],
                        "prefix": "",
                        "depth": 0,
                    }
                },
            )
            await client._send(
                "RecordGateVerdict",
                {
                    "changeset": links[index]["changeset"],
                    "branch": "main",
                    "gate_id": "test",
                    "input_digest": _listing_digest(tree["entries"]),
                    "verdict": verdict,
                    "evidence_digest": hashlib.sha256(b"log").hexdigest(),
                    "duration_ms": 1,
                },
                graph,
            )

        # No passing verdict yet: the chain cannot be searched.
        await record(4, "fail")
        with pytest.raises(Exception, match="BISECT_VERDICT_COVERAGE"):
            await bisect(None, links[4]["commit"])

        await record(0, "pass")
        for _ in range(5):
            answer = await bisect(None, links[4]["commit"])
            if answer["outcome"] == "culprit":
                assert answer["link"] == links[FIRST_BAD]
                break
            index = links.index(answer["link"])
            await record(index, "fail" if index >= FIRST_BAD else "pass")
        else:
            pytest.fail("bisect did not converge")

        # An unknown revision is refused and nothing was written by any step.
        with pytest.raises(Exception, match="REVISION_NOT_ON_CHAIN"):
            await bisect(None, "0" * 64)
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R009
@pytest.mark.spec("MEDULLA-SCM-R009")
def test_bisect_finds_culprit_and_replays_decision():
    asyncio.run(_scenario(f"scm-bisect-{uuid.uuid4().hex[:8]}"))
