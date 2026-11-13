"""Served `SourceIngest` record governance: MEDULLA-INGEST-WORKER-R900.5.

A record may carry the payload member `_governance`. The receipt of the batch
counts governed, refused and marked records; a refused record writes nothing.
"""

from __future__ import annotations

from typing import Any

import pytest

from .test_source_stream_versions_served import (
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)


def _governed(connector: str, page: int, previous: int | None, records: list) -> dict:
    batch = _batch(connector, page, previous, "unused")
    batch["records"] = records
    return batch


def _record(connector: str, record_id: str, governance: dict | None) -> dict:
    base = _batch(connector, 1, None, record_id)["records"][0]
    payload: dict[str, Any] = {"id": record_id, "name": record_id}
    if governance is not None:
        payload["_governance"] = governance
    return {**base, "record_id": record_id, "payload": payload}


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R900.5")
@pytest.mark.asyncio
async def test_receipt_counts_governed_refused_marked() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        batch = _governed(
            connector,
            2,
            None,
            [
                _record(
                    connector,
                    "marked",
                    {
                        "classification": "restricted",
                        "access_public": False,
                        "access_markings": ["pii"],
                        "access_read_roles": ["audit"],
                    },
                ),
                _record(
                    connector,
                    "internal",
                    {"classification": "internal", "access_public": False},
                ),
                # classification and the public flag disagree: refused.
                _record(
                    connector,
                    "conflict",
                    {"classification": "restricted", "access_public": True},
                ),
                _record(connector, "ungoverned", None),
            ],
        )
        receipt = await _submit(client, batch, "governance")
        assert receipt.governance.governed == 3
        assert receipt.governance.refused == 1
        assert receipt.governance.marked == 1
        # The refused record wrote nothing; the other three are admitted.
        assert receipt.affected_count == 3
        assert len(receipt.raw_admissions) == 3
    finally:
        await _close(client, graph)
