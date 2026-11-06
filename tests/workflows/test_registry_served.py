"""Workflow registry served over the wire (MEDULLA-DECISION-ENGINE-R166.6).

The journey registers one workflow, reads it back and reads an unknown identity
over the loopback engine with authentication. ``Register`` goes through the
mutation gateway; ``Read`` is answered by the catalog admin route.
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import coordination as gen

IDENTITY = {
    "workflow_id": "wf-summarize",
    "version": "1.0.0",
    "content_digest": "sha256:" + "a" * 64,
}


async def _journey() -> tuple[dict[str, Any], dict[str, Any], str]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        registered = await gen.send_workflow_registry(
            client,
            {
                "op": {
                    "op": "register",
                    "registration": {
                        "identity": IDENTITY,
                        "owner_scope": "",
                        "lifecycle": "active",
                    },
                }
            },
        )
        read = await gen.send_workflow_registry(
            client, {"op": {"op": "read", "identity": IDENTITY}}
        )
        unknown = dict(IDENTITY, version="9.9.9")
        refusal = ""
        try:
            await gen.send_workflow_registry(
                client, {"op": {"op": "read", "identity": unknown}}
            )
        except EngineResponseError as error:
            refusal = str(error)
        return registered.payload, read.payload, refusal
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R166.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R166.6")
def test_register_read_and_unknown_refusal(clean_graph):
    registered, read, refusal = asyncio.run(_journey())
    assert read == registered
    assert read["identity"] == IDENTITY
    assert read["lifecycle"] == "active"
    assert "WORKFLOW_IDENTITY_UNKNOWN" in refusal
