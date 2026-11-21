"""Served ontology induction: MEDULLA-INGEST-WORKER-R002.3 and R002.5.

A `proposals` batch of `SourceIngest` judges each submitted ontology proposal. The
per-proposal receipt, stored on the batch receipt node, carries the coverage and
redundancy scores the judgment used, and a resent batch replays its stored envelope,
so the scores read back unchanged.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest

from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    STREAM,
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
