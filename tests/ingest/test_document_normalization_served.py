"""Served document normalization through `SourceIngest`: MEDULLA-INGEST-WORKER-R003.12.

A document-class batch that fails `complete_document` is quarantined whole: the engine
commits one restricted `SourceQuarantine` evidence node and nothing of the batch is
projected. The quarantine keeps the stream's prior accepted checkpoint, a replay returns
the stored receipt without a second record, a failed quarantine commit never reports a
durable receipt, and a same-tenant caller the stored policy does not name reads nothing.
"""

from __future__ import annotations

import asyncio
import copy
import json
import os
import uuid
from typing import Any

import pytest
from conftest import TEST_AGENT_ID, TEST_SIGNER_KEY, request_context
from pydantic import TypeAdapter

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.ingestion import (
    SourceIngestStatusRequest,
    send_source_ingest_status,
)
from medulla.generated.source_ingestion import (
    QuarantineReason,
    SourceIngestionDisposition,
    SourceIngestionOutcome,
)

from .test_lsh_dedup_served import SOURCE_RECORD_ID, _document_client
from .test_source_stream_versions_served import (
    STREAM,
    _batch,
    _close,
    _connector,
    _submit,
)

CANARY = "canary sentence that must never reach the quarantine evidence"
VALID_TEXT = f"The ingest worker restarts after every upgrade. {CANARY}."
RULE_ID = "DOCUMENT_NORMALIZATION_FAILED"
QUARANTINE_LABEL = "SourceQuarantine"


def _document(record_id: str, number: int, text: str) -> dict[str, Any]:
    base = _batch(_connector(), 2, None, "unused")["records"][0]
    return {
        **base,
        "record_id": record_id,
        "provenance": {**base["provenance"], "source_uri": f"demo://doc/{record_id}"},
        "payload": {"id": number, "text": text},
    }


def _documents_batch(
    connector: str,
    page: int,
    previous: int | None,
    documents: list[tuple[str, str]],
) -> dict[str, Any]:
    batch = _batch(connector, page, previous, "unused")
    batch["records"] = [
        _document(record_id, number, text)
        for number, (record_id, text) in enumerate(documents, start=1)
    ]
    return batch


async def _outcome(client: MedullaClient, batch: dict[str, Any], key: str) -> Any:
    payload = await client._send(
        "SourceIngest",
        {"request": copy.deepcopy(batch)},
        idempotency_key=f"{key}:{uuid.uuid4().hex}",
    )
    return TypeAdapter(SourceIngestionOutcome).validate_python(payload)


async def _status(client: MedullaClient, connector: str) -> Any:
    return await send_source_ingest_status(
        client, SourceIngestStatusRequest(connector=connector, stream=STREAM)
    )


async def _quarantine_nodes(client: MedullaClient) -> list[str]:
    return [node for node, _ in await client.nodes.list_by_label(QUARANTINE_LABEL, 0)]


async def _projected(client: MedullaClient) -> tuple[list[str], list[str]]:
    """The document nodes that name a source record, and the knowledge atoms."""
    documents = []
    for node_id, _ in await client.nodes.list_by_label("Document", 0):
        props = await client.nodes.properties(node_id) or {}
        if props.get(SOURCE_RECORD_ID):
            documents.append(node_id)
    atoms = [node for node, _ in await client.nodes.list_by_label("KnowledgeAtom", 0)]
    return documents, atoms


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.12")
@pytest.mark.asyncio
async def test_document_batch_is_atomic() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        # One document is valid and one has no text: the whole batch is quarantined.
        batch = _documents_batch(
            connector, 2, None, [("good", VALID_TEXT), ("empty", "   ")]
        )
        outcome = await _outcome(client, batch, "atomic")
        assert outcome.outcome == "quarantined", outcome
        assert outcome.receipt.reason is QuarantineReason.NORMALIZATION_FAILED
        assert outcome.receipt.rule_id == RULE_ID
        # Nothing of the batch was projected, not even the valid document.
        documents, atoms = await _projected(client)
        assert documents == [] and atoms == []
        status = await _status(client, connector)
        assert status.accepted_checkpoint is None
        assert status.accepted_checkpoint_digest is None
        # The same batch with the empty document repaired is accepted whole.
        repaired = _documents_batch(
            connector, 2, None, [("good", VALID_TEXT), ("empty", "A repaired body.")]
        )
        receipt = await _submit(client, repaired, "atomic-repaired")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        documents, atoms = await _projected(client)
        assert len(documents) == 2 and atoms
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.12")
@pytest.mark.asyncio
async def test_first_batch_quarantine_and_replay() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        batch = _documents_batch(
            connector, 2, None, [("good", VALID_TEXT), ("empty", "")]
        )
        first = await _outcome(client, batch, "first")
        assert first.outcome == "quarantined", first
        assert first.receipt.prior_accepted_checkpoint is None
        assert first.receipt.restricted_evidence_ref.startswith("quarantine:")
        node_id = first.receipt.restricted_evidence_ref.removeprefix("quarantine:")
        assert await _quarantine_nodes(client) == [node_id]

        # The evidence holds reasons and digests only, never the source text.
        stored = await client.nodes.properties(node_id) or {}
        assert stored["reason"] == "normalization_failed"
        assert stored["rule_id"] == RULE_ID
        assert stored["replay"] == "replayable"
        assert CANARY not in json.dumps(stored, default=str)

        # A retry returns the stored receipt and writes no second record.
        again = await _outcome(client, batch, "again")
        assert again.outcome == "quarantined", again
        assert again.receipt == first.receipt
        assert await _quarantine_nodes(client) == [node_id]
        status = await _status(client, connector)
        assert status.accepted_checkpoint is None
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.12")
@pytest.mark.asyncio
async def test_quarantine_preserves_prior_checkpoint() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    try:
        accepted = _documents_batch(connector, 2, None, [("first", VALID_TEXT)])
        committed = await _submit(client, accepted, "prior")
        assert committed.disposition is SourceIngestionDisposition.COMMITTED
        before = await _status(client, connector)
        assert before.accepted_checkpoint is not None

        broken = _documents_batch(connector, 3, 2, [("second", "")])
        outcome = await _outcome(client, broken, "broken")
        assert outcome.outcome == "quarantined", outcome
        # The quarantine names the checkpoint the stream stays at, and moves nothing.
        assert outcome.receipt.prior_accepted_checkpoint == before.accepted_checkpoint
        after = await _status(client, connector)
        assert after.accepted_checkpoint == before.accepted_checkpoint
        assert after.accepted_checkpoint_digest == committed.accepted_checkpoint_digest
        assert after.live_set_digest == before.live_set_digest

        # The chain is intact: a repaired next page chains from the prior checkpoint.
        repaired = _documents_batch(
            connector, 3, 2, [("second", VALID_TEXT + " Again.")]
        )
        next_receipt = await _submit(client, repaired, "repaired")
        assert next_receipt.disposition is SourceIngestionDisposition.COMMITTED
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.12")
@pytest.mark.asyncio
async def test_failed_quarantine_commit_is_not_reported_durable() -> None:
    """A quarantine races an accepted batch on the same stream. Whichever way the commit
    fence falls, a returned quarantine receipt names a durable node and a refusal leaves
    none: no caller is told a quarantine is durable that was never committed."""
    for round_ in range(4):
        connector = _connector()
        client, graph = await _document_client(connector)
        rival = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            graph_name=graph,
        )
        try:
            quarantined = _documents_batch(connector, 2, None, [("bad", "")])
            accepted = _documents_batch(connector, 3, None, [("good", VALID_TEXT)])
            results = await asyncio.gather(
                _outcome(client, quarantined, f"race-q-{round_}"),
                _outcome(rival, accepted, f"race-a-{round_}"),
                return_exceptions=True,
            )
            nodes = await _quarantine_nodes(client)
            if isinstance(results[0], EngineResponseError):
                assert "STALE_CONTENT_VERSION" in str(results[0]), results[0]
                assert nodes == [], "a refused quarantine left evidence"
            else:
                assert not isinstance(results[0], BaseException), results[0]
                assert results[0].outcome == "quarantined", results[0]
                node_id = results[0].receipt.restricted_evidence_ref.removeprefix(
                    "quarantine:"
                )
                assert nodes == [node_id], "a reported quarantine is not durable"
            # The accepted batch is unaffected by a quarantine, and a refusal is typed.
            if isinstance(results[1], EngineResponseError):
                assert "STALE_CONTENT_VERSION" in str(results[1]), results[1]
        finally:
            await rival.close()
            await _close(client, graph)


async def _hidden(call: Any) -> bool:
    """True when the read is refused or returns nothing."""
    try:
        value = await call
    except EngineResponseError:
        return True
    return not value


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.12")
@pytest.mark.asyncio
async def test_quarantine_reads_require_policy() -> None:
    connector = _connector()
    client, graph = await _document_client(connector)
    outsider: MedullaClient | None = None
    try:
        batch = _documents_batch(connector, 2, None, [("bad", "")])
        outcome = await _outcome(client, batch, "policy")
        assert outcome.outcome == "quarantined", outcome
        node_id = outcome.receipt.restricted_evidence_ref.removeprefix("quarantine:")
        assert await client.nodes.properties(node_id), (
            "the submitter reads its evidence"
        )

        # A principal of the same tenant may read the graph but the stored policy of the
        # evidence does not name it.
        tag = uuid.uuid4().hex[:8]
        agent_id, role = f"service:quarantine-outsider-{tag}", f"suite:quarantine-{tag}"
        await client.rbac.add_role(role)
        await client.rbac.add_grant(role, {"Graph": graph}, "Read")
        await client.consensus.register_identity(
            agent_id,
            "Agent",
            [],
            [role],
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
        outsider = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=agent_id),
            graph_name=graph,
        )
        assert await _hidden(outsider.nodes.properties(node_id))
        assert await _hidden(outsider.nodes.list_by_label(QUARANTINE_LABEL, 0))
        # Neither a resubmission nor the raw reference gives the outsider the receipt.
        with pytest.raises(EngineResponseError):
            await _outcome(outsider, batch, "policy-outsider")
        assert await _hidden(
            outsider.nodes.properties(outcome.receipt.restricted_evidence_ref)
        )
    finally:
        if outsider is not None:
            await outsider.close()
        await _close(client, graph)
