"""Served ontology induction: MEDULLA-INGEST-WORKER-R002.3 and R002.5.

A `proposals` batch of `SourceIngest` judges each submitted ontology proposal. The
per-proposal receipt, stored on the batch receipt node, carries the coverage and
redundancy scores the judgment used, and a resent batch replays its stored envelope,
so the scores read back unchanged.
"""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries

from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    STREAM,
    _batch as _records_batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)


def _submission(proposal_id: str, declared: list[str], observed: list[str]) -> dict:
    return {
        "proposal": {
            "proposal_id": proposal_id,
            "classes": [{"name": "Thing", "properties": declared}],
            "properties": [],
            "provenance": {"source_ref": "worker", "induced_by": "model"},
        },
        "observed_terms": observed,
    }


def _batch(connector: str, submissions: list[dict]) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "proposals",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "provider_checkpoint": {"stream": STREAM, "position": {"page": 1}},
        "extension": {"proposals": submissions},
    }


def _proposals_of(props: dict[str, Any]) -> dict[str, dict[str, Any]]:
    items = props["proposals"]
    return {item["proposal_id"]: item for item in items}


# spec: MEDULLA-INGEST-WORKER-R002.5
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R002.5")
@pytest.mark.asyncio
async def test_receipt_reports_scores_and_replays() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        suffix = uuid.uuid4().hex[:8]
        passing, failing = f"p-pass-{suffix}", f"p-fail-{suffix}"
        batch = _batch(
            connector,
            [
                _submission(passing, ["name", "age"], ["name", "age"]),
                _submission(failing, ["name"], ["name", "age", "city", "zip"]),
            ],
        )
        first = await _submit(client, batch, "scores")
        assert first.disposition is SourceIngestionDisposition.COMMITTED

        receipt_node = await client.nodes.properties(first.receipt_id)
        assert receipt_node is not None
        items = _proposals_of(receipt_node)
        assert items[passing]["state"] == "admitted"
        assert items[passing]["coverage"] == 1.0
        assert items[passing].get("refusal") is None
        assert items[failing]["state"] == "draft"
        assert items[failing]["refusal"] == "PROPOSAL_NOT_ADMITTED"
        assert items[failing]["failing_check"] == "coverage"
        assert items[failing]["coverage"] < 0.8

        # The same two scores are on the stored proposal nodes.
        stored = await client.nodes.properties(f"ontology_proposal:{failing}")
        assert stored is not None
        assert stored["coverage"] == items[failing]["coverage"]
        assert stored["redundancy"] == items[failing]["redundancy"]

        # A resent batch replays its stored envelope: the scores read back unchanged.
        replay = await _submit(client, batch, "scores-replay")
        assert replay.disposition is SourceIngestionDisposition.REPLAYED
        assert replay.receipt_id == first.receipt_id
        again = await client.nodes.properties(first.receipt_id)
        assert _proposals_of(again) == items
    finally:
        await _close(client, graph)


def _proposal_declaration(connector: str, proposal_id: str) -> ConnectorPackEntryContent:
    body = {
        "connector": connector,
        "schema_mappings": {
            "item": {
                "fields": {"id": "identifier", "name": "label"},
                # A pack names a proposed class as proposal:<id>#<class>.
                "ontology_class": f"proposal:{proposal_id}#Thing",
            }
        },
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        output_schema=b'{"type":"object"}',
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


async def _proposal_client(connector: str, proposal_id: str) -> tuple[MedullaClient, str]:
    admin = await connect()
    graph = f"induction-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(
            admin, connector, [_proposal_declaration(connector, proposal_id)]
        )
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


# spec: MEDULLA-INGEST-WORKER-R002.7
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R002.7")
@pytest.mark.asyncio
async def test_draft_proposal_takes_no_instances() -> None:
    connector = _connector()
    proposal_id = f"p-{uuid.uuid4().hex[:8]}"
    client, graph = await _proposal_client(connector, proposal_id)
    try:
        # The proposal does not exist yet: the records batch refuses and writes nothing.
        records = _records_batch(connector, 2, None, "first")
        with pytest.raises(Exception, match="PROPOSAL_NOT_ADMITTED"):
            await _submit(client, records, "absent")

        # A draft proposal takes no instances either.
        draft = _batch(
            connector, [_submission(proposal_id, ["name"], ["name", "age", "city", "zip"])]
        )
        await _submit(client, draft, "draft")
        with pytest.raises(Exception, match="PROPOSAL_NOT_ADMITTED"):
            await _submit(client, records, "draft-instances")
        assert list(await client.nodes.list_by_label("Thing", 0)) == []
    finally:
        await _close(client, graph)


# spec: MEDULLA-INGEST-WORKER-R002.6
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R002.6")
@pytest.mark.asyncio
async def test_passing_proposal_admitted_failing_stays_draft() -> None:
    connector = _connector()
    suffix = uuid.uuid4().hex[:8]
    passing, failing = f"p-pass-{suffix}", f"p-fail-{suffix}"
    # The fixture is two fixed proposals with fixed observed terms; no model call is involved.
    client, graph = await _proposal_client(connector, failing)
    try:
        batch = _batch(
            connector,
            [
                _submission(passing, ["name", "age"], ["name", "age"]),
                _submission(failing, ["name"], ["name", "age", "city", "zip"]),
            ],
        )
        receipt = await _submit(client, batch, "fixture")
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED

        # The receipt scores and states: the passing proposal is admitted, the failing one stays
        # a draft with the coverage check named.
        receipt_node = await client.nodes.properties(receipt.receipt_id)
        assert receipt_node is not None
        items = _proposals_of(receipt_node)
        assert items[passing]["state"] == "admitted"
        assert items[passing]["coverage"] == 1.0
        assert items[failing]["state"] == "draft"
        assert items[failing]["failing_check"] == "coverage"
        assert items[failing]["coverage"] < 0.8

        # The stored proposal nodes carry the same scores.
        stored = await client.nodes.properties(f"ontology_proposal:{failing}")
        assert stored is not None
        assert stored["coverage"] == items[failing]["coverage"]

        # An instance batch against the draft is refused, so no instance is written.
        with pytest.raises(Exception, match="PROPOSAL_NOT_ADMITTED"):
            await _submit(client, _records_batch(connector, 2, None, "first"), "draft-instances")
        assert list(await client.nodes.list_by_label("Thing", 0)) == []
    finally:
        await _close(client, graph)
