"""The landing fixture shared by the served lifecycle journeys.

MEDULLA-SDLC-R015.4, R015.6 and R017.7 prove that a result counts only after the
gates and the landing. This module is the one way they land a change: a branch of
a repository graph, a changeset submitted through the served ``SubmitChangeset``,
the sealed gate verdict recorded for that changeset's tree, the landing policy of
``main`` and the served ``BranchMerge`` that publishes the branch. It writes no
spec, plan or task file any other way.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any

import msgpack
from conftest import TEST_AGENT_ID, TEST_TENANT

MODE = 0o100644
GATE = "spec-standard"
CONTENT_DOMAIN = b"eg/ingest-readback/node/v1"


def body(payload: Any) -> Any:
    """Decode a served answer that is either a msgpack body or already a value."""
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def listing_digest(entries: list[dict]) -> str:
    """The tree digest the landing gate judges (``tree_listing_digest``)."""
    text = "".join(
        f"{e['path']}\t{e['blob_digest']}\t{e['mode']:o}\n"
        for e in sorted(entries, key=lambda e: e["path"])
    )
    return hashlib.sha256(text.encode()).hexdigest()


def content_digest(properties: dict) -> str:
    """The digest a proposal cites for a stored record (the ingest read-back digest)."""
    from medulla.generated.digest import framed_sha256

    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return framed_sha256(CONTENT_DOMAIN, [compact.encode()])


async def add_node(client: Any, graph: str, node_id: str, properties: dict) -> None:
    await client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        graph,
    )


async def cited_fact(client: Any, graph: str, suffix: str) -> dict:
    """One stored code fact and the evidence entry that cites it."""
    record_id = f"fact:landing-{suffix}"
    properties = {"type": "CodeFact", "name": f"landing-{suffix}"}
    await add_node(client, graph, record_id, properties)
    return {
        "kind": "code_graph_fact",
        "record_id": record_id,
        "digest": content_digest(properties),
    }


class LandingRepo:
    """One repository graph whose ``main`` is guarded by a landing policy."""

    def __init__(self, owner: Any, graph: str, gate: str) -> None:
        self.owner = owner
        self.graph = graph
        self.gate = gate
        self._count = 0

    @classmethod
    async def create(
        cls, owner: Any, graph: str, *writers: str, gate: str = GATE
    ) -> LandingRepo:
        """Create ``graph``, grant ``writers`` writes, require ``gate`` on ``main``."""
        await owner.tenants.create(graph, "Global")
        for principal in (TEST_AGENT_ID, *writers):
            await owner._send(
                "SetPathGrant",
                {
                    "request": {
                        "repo": graph,
                        "principal": principal,
                        "prefix": "",
                        "read": True,
                        "write": True,
                    }
                },
            )
        await owner.landing_policy_put(
            {"policy": {"repo": graph, "branch": "main", "required_gates": [gate]}}
        )
        return cls(owner, graph, gate)

    def _key(self, label: str) -> str:
        self._count += 1
        return f"{self.graph}-{label}-{self._count}"

    async def branch(self, name: str) -> None:
        await self.owner._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": self.graph, "name": name}
        )

    async def submit_request(
        self,
        client: Any,
        branch: str,
        path: str,
        data: bytes,
        message: str,
        base_head: str | None = None,
        op: str = "add",
    ) -> dict:
        digest = await client.blob.store(data)
        return {
            "request": {
                "repo": self.graph,
                "branch": branch,
                "base_head": base_head,
                "message": message,
                "paths": [
                    {
                        "path": path,
                        "op": op,
                        "blob": {"digest": digest, "size": len(data)},
                        "mode": MODE,
                        "renamed_from": None,
                    }
                ],
                "trailers": None,
                "run_trace_id": None,
            }
        }

    async def submit(
        self,
        client: Any,
        branch: str,
        path: str,
        data: bytes,
        message: str,
        base_head: str | None = None,
        op: str = "add",
    ) -> dict:
        """Submit one changeset through the served ``SubmitChangeset``."""
        request = await self.submit_request(
            client, branch, path, data, message, base_head, op
        )
        return await client._send(
            "SubmitChangeset", request, idempotency_key=self._key("submit")
        )

    async def tree_digest(self, branch: str, head: str) -> str:
        tree = await self.owner._send(
            "ReadTree",
            {
                "request": {
                    "repo": self.graph,
                    "branch": branch,
                    "rev": head,
                    "prefix": "",
                    "depth": 0,
                }
            },
        )
        return listing_digest(tree["entries"])

    async def record(self, receipt: dict, branch: str, verdict: str = "pass") -> dict:
        """Record the gate's verdict for the tree the receipt left on ``branch``."""
        changeset = receipt["changeset"]["id"]
        return body(
            await self.owner._send(
                "RecordGateVerdict",
                {
                    "changeset": changeset,
                    "branch": branch,
                    "gate_id": self.gate,
                    "input_digest": await self.tree_digest(
                        branch, receipt["branch_head"]
                    ),
                    "verdict": verdict,
                    "evidence_digest": hashlib.sha256(
                        f"log:{changeset}".encode()
                    ).hexdigest(),
                    "duration_ms": 7,
                },
                self.graph,
            )
        )

    def merge_request(self, source: str, target: str = "main") -> dict:
        return {
            "tenant": TEST_TENANT,
            "graph": self.graph,
            "source": source,
            "target": target,
        }

    async def merge(self, source: str, target: str = "main") -> dict:
        """Publish ``source`` onto ``target``; the landing policy decides."""
        return await self.owner._send("BranchMerge", self.merge_request(source, target))

    async def land(
        self,
        client: Any,
        branch: str,
        path: str,
        data: bytes,
        message: str,
        verdict: str = "pass",
    ) -> tuple[dict, dict]:
        """Branch, submit, record the verdict, merge: the way a change reaches main."""
        await self.branch(branch)
        receipt = await self.submit(client, branch, path, data, message)
        await self.record(receipt, branch, verdict)
        return receipt, await self.merge(branch)

    async def has_node(self, node_id: str) -> bool:
        return bool(
            await self.owner._send("HasNode", {"node_id": node_id}, graph=self.graph)
        )

    async def close(self) -> None:
        await self.owner.tenants.delete(self.graph)
