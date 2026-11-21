"""MEDULLA-BRANCH-MERGE-R009: the landing policy gates the merge publish step.

A branch changeset is merged into ``main`` while ``main`` carries a landing policy that
requires a gate with no recorded verdict. The merge refuses ``LANDING_GATE_BLOCKED``
and the
base graph reads exactly as before. Lowering the policy needs an operator approval.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

_DOC = b"\x81\xa4type\xa3Doc"


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/gate-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )
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
        node = f"landing-{stem}"
        await client._send(
            "BranchWrite",
            {
                "tenant": TEST_TENANT,
                "graph": graph,
                "branch": branch,
                "mutation": {
                    "method": "AddNode",
                    "params": {"node_id": node, "properties_msgpack": _DOC},
                },
            },
            idempotency_key=f"kl-{stem}",
        )
        data = f"landing {stem}".encode()
        digest = await client.blob.store(data)
        await client._send(
            "SubmitChangeset",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "base_head": None,
                    "message": "add a\n\nSpec: MEDULLA-BRANCH-MERGE-R009",
                    "paths": [
                        {
                            "path": "a",
                            "op": "add",
                            "blob": {"digest": digest, "size": len(data)},
                            "mode": 0o100644,
                            "renamed_from": None,
                        }
                    ],
                    "trailers": None,
                    "run_trace_id": None,
                }
            },
            idempotency_key=f"kls-{stem}",
        )
        policy = await client.landing_policy_put(
            {"policy": {"repo": graph, "branch": "main", "required_gates": ["fmt"]}}
        )
        assert policy["node_id"] == f"landing_policy:{graph}:main"
        assert policy["lowered"] is False

        merge = {
            "tenant": TEST_TENANT,
            "graph": graph,
            "source": branch,
            "target": "main",
        }
        with pytest.raises(Exception, match="LANDING_GATE_BLOCKED"):
            await client._send("BranchMerge", merge)
        assert not await client._send("HasNode", {"node_id": node}, graph=graph)

        # Dropping the required gate lowers the policy: it needs an operator approval.
        lowered = {"policy": {"repo": graph, "branch": "main", "required_gates": []}}
        with pytest.raises(Exception, match="LANDING_POLICY_APPROVAL_REQUIRED"):
            await client.landing_policy_put(lowered)
        approved = await client.landing_policy_put(
            {
                **lowered,
                "approval": {
                    "approver": "owner",
                    "approved_at_ms": 1,
                    "reason": "drop the gate",
                },
            }
        )
        assert approved["lowered"] is True
        receipt = await client._send("BranchMerge", merge)
        assert receipt["outcome"] == "fast_forward"
        assert await client._send("HasNode", {"node_id": node}, graph=graph)
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R009
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R009")
def test_failing_gate_blocks_merge_and_target_unchanged():
    asyncio.run(_scenario(f"landing-gate-{uuid.uuid4().hex[:8]}"))
