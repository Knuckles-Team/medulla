"""MEDULLA-SCM-R012: agent principals and run provenance of ``SubmitChangeset``.

A delegated agent (its verified agent differs from its principal) must name a
run it can read in the repository graph. The sealed changeset records the agent
identity and the run id; a human may omit the run.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import msgpack
import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644


def _request(
    graph: str, branch: str, base: str | None, blob: tuple[str, int], run: str | None
) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "base_head": base,
            "message": "agent change\n\nSpec: MEDULLA-SCM-R012",
            "paths": [
                {
                    "path": "src/a.rs",
                    "op": "add" if base is None else "modify",
                    "blob": {"digest": blob[0], "size": blob[1]},
                    "mode": MODE,
                    "renamed_from": None,
                }
            ],
            "trailers": None,
            "run_trace_id": run,
        }
    }


async def _scenario(graph: str) -> None:
    human = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    agent = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="human:delegator"),
        timeout=30,
    )
    try:
        await human.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/provenance-{stem}"
        await human._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )
        await human._send(
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
        # The repository graph holds a run-trace node and a node of another kind.
        for node_id, kind in (
            (f"run-{stem}", "run_trace"),
            (f"doc-{stem}", "document"),
        ):
            await human._send(
                "AddNode",
                {
                    "node_id": node_id,
                    "properties_msgpack": msgpack.packb({"kind": kind}),
                },
                graph=graph,
            )
        data = f"fn main() {{}} {stem}".encode()
        blob = (await agent.blob.store(data), len(data))

        # A delegated agent without a run, and with an unknown or wrong-kind run, refuses.
        for run, code in (
            (None, "AGENT_RUN_TRACE_MISSING"),
            (f"absent-{stem}", "RUN_TRACE_UNKNOWN"),
            (f"doc-{stem}", "RUN_TRACE_UNKNOWN"),
        ):
            with pytest.raises(Exception, match=code):
                await agent._send(
                    "SubmitChangeset",
                    _request(graph, branch, None, blob, run),
                    idempotency_key=f"refuse-{code}-{stem}",
                )

        # With a run it created, the sealed record carries the agent and the run.
        receipt = await agent._send(
            "SubmitChangeset",
            _request(graph, branch, None, blob, f"run-{stem}"),
            idempotency_key=f"land-{stem}",
        )
        assert receipt["changeset"]["agent_identity"] == TEST_AGENT_ID
        assert receipt["changeset"]["run_trace_id"] == f"run-{stem}"

        # A human may omit the run.
        data = f"fn main() {{ 1 }} {stem}".encode()
        blob = (await human.blob.store(data), len(data))
        landed = await human._send(
            "SubmitChangeset",
            _request(graph, branch, receipt["branch_head"], blob, None),
            idempotency_key=f"human-{stem}",
        )
        assert landed["changeset"]["agent_identity"] is None
        assert landed["changeset"]["run_trace_id"] is None
        await human.tenants.delete(graph)
    finally:
        await agent.close()
        await human.close()


# spec: MEDULLA-SCM-R012
@pytest.mark.spec("MEDULLA-SCM-R012")
def test_changeset_links_to_run_trace():
    asyncio.run(_scenario(f"scm-provenance-{uuid.uuid4().hex[:8]}"))
