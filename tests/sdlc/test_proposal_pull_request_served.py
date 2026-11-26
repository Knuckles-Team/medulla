"""Proposal accept opens a pull request (MEDULLA-SDLC-R017.8).

L4: every call goes through the running engine. The test engine has no connector
gateway, so the `open_pull_request` call fails with the typed
`CONNECTOR_PORT_UNAVAILABLE`: the accept still commits, the receipt carries the typed
error and the proposal records it. The caller passes only `head` and `base`; the engine
resolves the connector server from the repository node's registered source
connection, and a repository with none gets `SOURCE_CONNECTION_UNKNOWN` on the
receipt.
"""

from __future__ import annotations

import json
import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

from medulla.generated.digest import framed_sha256

GRAPH = "__commons__"
TARGET = {"head": "proposal/1", "base": "main"}


def _body(payload: Any) -> dict[str, Any]:
    if isinstance(payload, (bytes, bytearray)):
        return dict(msgpack.unpackb(payload, raw=False))
    return dict(payload)


async def _connect() -> Any:
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _open_proposal(client: Any, suffix: str) -> tuple[str, str]:
    project = f"project-{suffix}"
    mode = {"project": project, "mode": "development"}
    await client._send("ProjectModeSet", {"request": mode}, GRAPH)
    record = f"fact:pr-{suffix}"
    properties = {"type": "CodeFact", "name": f"pr-{suffix}"}
    await client._send(
        "AddNode",
        {
            "node_id": record,
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        GRAPH,
    )
    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    digest = framed_sha256(b"eg/ingest-readback/node/v1", [compact.encode()])
    proposal = {
        "project": project,
        "kind": "row_add",
        "target": "MEDULLA-SDLC-R017.8",
        "statement": "Add the pull request row.",
        "evidence": [
            {"kind": "code_graph_fact", "record_id": record, "digest": digest}
        ],
    }
    put = _body(await client._send("ProposalPut", {"proposal": proposal}, GRAPH))
    return project, str(put["node_id"])


async def _decide(client: Any, proposal_id: str, decision: str, **extra: Any) -> Any:
    request = {
        "id": proposal_id,
        "decision": decision,
        "reviewer": "reviewer-1",
        "reason": f"{decision} after reading the evidence",
        **extra,
    }
    return _body(await client._send("ProposalDecide", {"request": request}, GRAPH))


async def _register_connection(client: Any, project: str) -> None:
    """Register the repository node's source connection (server and repo)."""
    properties = {
        "type": "Repository",
        "name": project,
        "tenant_id": TEST_TENANT,
        "source_connection_server": "scm",
        "source_connection_repo": "org/demo",
    }
    await client._send(
        "AddNode",
        {
            "node_id": f"repository:{project}",
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        GRAPH,
    )


async def _node(client: Any, node_id: str) -> dict[str, Any]:
    return _body(await client._send("GetNodeProperties", {"node_id": node_id}, GRAPH))


@pytest.mark.spec("MEDULLA-SDLC-R017.8")
@pytest.mark.asyncio
async def test_accept_commits_and_a_failed_pull_request_is_a_typed_error(
    start_medulla_server: Any,
) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        _, first = await _open_proposal(client, suffix)

        # A malformed target refuses before anything is written.
        broken = {**TARGET, "head": " "}
        with pytest.raises(Exception, match="INVALID_ARGUMENT"):
            await _decide(client, first, "approve", pull_request=broken)
        assert (await _node(client, first))["status"] == "proposed"

        # No registered source connection: the accept commits and the receipt says why.
        # spec: MEDULLA-SDLC-R017.8
        receipt = await _decide(client, first, "approve", pull_request=TARGET)
        assert receipt["state"] == "approved"
        assert "pull_request" not in receipt
        assert receipt["pull_request_error"]["code"] == "SOURCE_CONNECTION_UNKNOWN"
        stored = await _node(client, first)
        assert stored["status"] == "approved"
        assert stored["pull_request_error_code"] == "SOURCE_CONNECTION_UNKNOWN"

        # With a registered connection the engine resolves the server itself; the test
        # engine has no gateway, so the call fails with the typed port error.
        project, third = await _open_proposal(client, suffix + "c")
        await _register_connection(client, project)
        receipt = await _decide(client, third, "approve", pull_request=TARGET)
        assert receipt["state"] == "approved"
        assert "pull_request" not in receipt
        assert receipt["pull_request_error"]["code"] == "CONNECTOR_PORT_UNAVAILABLE"
        stored = await _node(client, third)
        assert stored["status"] == "approved"
        assert stored["pull_request"] is None
        assert stored["pull_request_error_code"] == "CONNECTOR_PORT_UNAVAILABLE"

        # A rejection opens no pull request and records none.
        _, second = await _open_proposal(client, suffix + "b")
        rejected = await _decide(client, second, "reject", pull_request=TARGET)
        assert rejected["state"] == "rejected"
        assert "pull_request" not in rejected
        assert "pull_request_error" not in rejected
    finally:
        await client.close()
