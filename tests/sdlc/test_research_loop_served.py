"""MEDULLA-SDLC-R016.2: findings are cited entries in ``research.md``.

L3: every call goes through the running engine. A research entry is an
``edit_plan_section`` proposal whose target starts with ``research:``; each
citation is a ``research_citation`` evidence whose record is an ingested source
node, and the entry text holds one ``Source: <record_id> (read at <ms>)`` line
per citation.
"""

from __future__ import annotations

import json
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


def _content_digest(properties: dict) -> str:
    from medulla.generated.digest import framed_sha256

    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return framed_sha256(b"eg/ingest-readback/node/v1", [compact.encode()])


async def _entry(client, project: str, statement: str, evidence: list[dict]) -> dict:
    proposal = {
        "project": project,
        "kind": "edit_plan_section",
        "target": "research:planunknown:demo:plan.md#L4",
        "statement": statement,
        "evidence": evidence,
    }
    return _body(await client._send("ProposalPut", {"proposal": proposal}, GRAPH))


# spec: MEDULLA-SDLC-R016.2
@pytest.mark.spec("MEDULLA-SDLC-R016.2")
@pytest.mark.asyncio
async def test_uncited_entry_is_refused(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        await client._send(
            "ProjectModeSet",
            {"request": {"project": project, "mode": "development"}},
            GRAPH,
        )
        source = f"source:research-{suffix}"
        properties = {"type": "WebSnapshot", "name": f"page-{suffix}"}
        await client._send(
            "AddNode",
            {
                "node_id": source,
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
            GRAPH,
        )
        citation = {
            "kind": "research_citation",
            "record_id": source,
            "digest": _content_digest(properties),
        }
        # No research_citation evidence at all.
        with pytest.raises(Exception, match="RESEARCH_ENTRY_UNCITED"):
            await _entry(client, project, "The finding.", [])
        # Evidence, but the text holds no Source line for it.
        with pytest.raises(Exception, match="RESEARCH_ENTRY_UNCITED"):
            await _entry(client, project, "The finding.", [citation])
        # A cited entry is accepted.
        statement = f"The finding.\nSource: {source} (read at 1700000000000)"
        receipt = await _entry(client, project, statement, [citation])
        assert receipt["evidence"] == 1
    finally:
        await client.close()
