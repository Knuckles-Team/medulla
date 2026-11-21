"""Project mode record (MEDULLA-SDLC-R012.8).

L4: every call goes through the running engine. A project with no record reads
as `reference`; a move to `evolving` needs a full operator approval.
"""

from __future__ import annotations

import os
import uuid

import msgpack
import pytest
from conftest import request_context

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


async def _connect():
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _set(client, project: str, mode: str, approval: dict | None = None) -> dict:
    request: dict = {"project": project, "mode": mode}
    if approval is not None:
        request["approval"] = approval
    return _body(await client._send("ProjectModeSet", {"request": request}, GRAPH))


async def _stored_mode(client, node_id: str) -> str:
    raw = await client._send("GetNodeProperties", {"node_id": node_id}, GRAPH)
    return _body(raw)["mode"]


@pytest.mark.spec("MEDULLA-SDLC-R012.8")
@pytest.mark.asyncio
async def test_evolving_needs_operator_approval_and_defaults_to_reference(
    start_medulla_server,
) -> None:
    client = await _connect()
    try:
        project = f"project-{uuid.uuid4().hex[:12]}"

        # No record reads as reference: the first receipt names it as previous.
        receipt = await _set(client, project, "development")
        assert receipt["previous"] == "reference"
        assert receipt["mode"] == "development"
        assert receipt["node_id"].startswith("projectmode:")
        assert await _stored_mode(client, receipt["node_id"]) == "development"

        with pytest.raises(Exception, match="MODE_APPROVAL_REQUIRED"):
            await _set(client, project, "evolving")
        assert await _stored_mode(client, receipt["node_id"]) == "development"

        approval = {
            "approver": "operator",
            "approved_at_ms": 1_700_000_000_000,
            "reason": "goals reviewed",
        }
        receipt = await _set(client, project, "evolving", approval)
        assert receipt["previous"] == "development"
        assert receipt["mode"] == "evolving"
        assert await _stored_mode(client, receipt["node_id"]) == "evolving"
    finally:
        await client.close()
