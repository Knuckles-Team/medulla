"""Restricted source nodes through generic reads: MEDULLA-INGEST-WORKER-R003.17.1.

A quarantine node and a transcript thread are governed source records readable
only under their stored policy action. The row-level read gate decides them by
their source access properties, so a principal that holds a graph Read grant and
the `kg:read` scope still sees neither through GetNodeProperties, a label
listing or a query. The role named exactly like the action is the row
authority; the typed read adds the scope.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import TEST_AGENT_ID, TEST_SIGNER_KEY, request_context

from medulla.client import EngineResponseError, MedullaClient, SyncMedullaClient

from .test_document_normalization_served import _documents_batch, _outcome
from .test_lsh_dedup_served import _document_client
from .test_source_batch_kind_served import _transcript_batch
from .test_source_stream_versions_served import (
    _close,
    _connector,
    _ingest_client,
    _submit,
)

QUARANTINE_ACTION = "source:quarantine:read"
TRANSCRIPT_ACTION = "source:transcript:read"


def _register(graph: str, agent_id: str, roles: list[str]) -> None:
    """Register `agent_id` as an Agent holding `roles`, all of them with graph Read."""
    admin = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        for role in roles:
            try:
                admin.rbac.add_role(role)
            except EngineResponseError:
                pass  # another case on this engine already created it
            admin.rbac.add_grant(role, {"Graph": graph}, "Read")
        admin.consensus.register_identity(
            agent_id,
            "Agent",
            [],
            roles,
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
    finally:
        admin.close()


async def _as(graph: str, agent_id: str, scopes: list[str]) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent_id, scopes=scopes),
        graph_name=graph,
    )


async def _sees(
    client: MedullaClient, label: str, node_id: str
) -> tuple[Any, list[str], list[Any]]:
    """What the generic reads return for `node_id`: its properties, the label
    listing and the rows of a query over the label."""
    properties = await client.nodes.properties(node_id)
    listed = [node for node, _ in await client.nodes.list_by_label(label, 0)]
    rows = (await client.query.uql(f"MATCH (:{label})")).get("rows", [])
    return properties, listed, rows


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.17.1")
@pytest.mark.asyncio
async def test_kg_read_cannot_see_quarantine_through_generic_reads() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    suffix = uuid.uuid4().hex[:8]
    plain = f"service:plain-{suffix}"
    holder = f"service:holder-{suffix}"
    readers: list[MedullaClient] = []
    try:
        outcome = await _outcome(
            client, _documents_batch(connector, 2, None, [("bad", "")]), "restricted"
        )
        assert outcome.outcome == "quarantined", outcome
        node_id = outcome.receipt.restricted_evidence_ref.removeprefix("quarantine:")
        assert await client.nodes.properties(node_id), "the System submitter reads it"

        _register(graph, plain, [f"suite:plain-{suffix}"])
        reader = await _as(graph, plain, ["kg:read"])
        readers.append(reader)
        properties, listed, rows = await _sees(reader, "SourceQuarantine", node_id)
        assert not properties
        assert node_id not in listed
        assert not rows

        _register(graph, holder, [QUARANTINE_ACTION])
        allowed = await _as(graph, holder, ["kg:read", QUARANTINE_ACTION])
        readers.append(allowed)
        properties, listed, rows = await _sees(allowed, "SourceQuarantine", node_id)
        assert properties and properties["type"] == "SourceQuarantine"
        assert node_id in listed
        assert rows
    finally:
        for reader in readers:
            await reader.close()
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.17.1")
@pytest.mark.asyncio
async def test_kg_read_cannot_see_transcripts_through_generic_reads() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    suffix = uuid.uuid4().hex[:8]
    plain = f"service:plain-{suffix}"
    holder = f"service:holder-{suffix}"
    readers: list[MedullaClient] = []
    try:
        concept = f"concept-{suffix}"
        await client.nodes.add(concept, {"type": "Concept"})
        await _submit(
            client,
            _transcript_batch(connector, 1, None, concept, "claude"),
            "restricted",
        )
        [(thread_id, _)] = await client.nodes.list_by_label("Transcript", 0)
        [(segment_id, _), *_] = await client.nodes.list_by_label("TranscriptSegment", 0)

        _register(graph, plain, [f"suite:plain-{suffix}"])
        reader = await _as(graph, plain, ["kg:read"])
        readers.append(reader)
        for label, node_id in (
            ("Transcript", thread_id),
            ("TranscriptSegment", segment_id),
        ):
            properties, listed, rows = await _sees(reader, label, node_id)
            assert not properties, label
            assert node_id not in listed, label
            assert not rows, label

        _register(graph, holder, [TRANSCRIPT_ACTION])
        allowed = await _as(graph, holder, ["kg:read", TRANSCRIPT_ACTION])
        readers.append(allowed)
        properties, listed, _ = await _sees(allowed, "TranscriptSegment", segment_id)
        assert properties and properties["text"].startswith("text of m")
        assert segment_id in listed
    finally:
        for reader in readers:
            await reader.close()
        await _close(client, graph)
