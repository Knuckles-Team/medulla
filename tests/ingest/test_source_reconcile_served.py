"""Served enumeration reconcile receipt: MEDULLA-INGEST-WORKER-R903.5.

A complete `enumeration` batch that omits a held record plans that record's removal
after the grace window. The receipt reports the digest of the listed identifiers, the
count of held records the run no longer lists, the count already deleted (none at
admission) and the principal that lifted the mass-delete guard (none here).
"""

from __future__ import annotations

import copy
import hashlib
import uuid
from typing import Any

import pytest

from .test_source_stream_versions_served import (
    STREAM,
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)


def _ids_digest(record_ids: list[str]) -> str:
    """Digest of the sorted identifier digests of a whole run."""
    digests = sorted(
        hashlib.sha256(f"{STREAM}\0{record_id}".encode()).digest()
        for record_id in dict.fromkeys(record_ids)
    )
    hasher = hashlib.sha256()
    hasher.update(b"eg/source-enumeration-ids/v1")
    for digest in digests:
        hasher.update(digest)
    return hasher.hexdigest()


def _two_records(connector: str) -> dict[str, Any]:
    batch = _batch(connector, 1, None, "one")
    second = copy.deepcopy(batch["records"][0])
    second["record_id"] = "item-2"
    second["payload"] = {"id": 2, "name": "two"}
    second["provenance"]["source_uri"] = "demo://items/item-2"
    batch["records"].append(second)
    return batch


def _enumeration(connector: str, listed: list[str]) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "enumeration",
        "mode": "reconcile",
        "strict_schema": True,
        "records": [],
        "provider_checkpoint": {"stream": STREAM, "position": {"page": 2}},
        "extension": {
            "enumeration": {
                "run_id": f"run-{uuid.uuid4().hex[:8]}",
                "page": 0,
                "last_page": True,
                "complete": True,
                "ids_digest": _ids_digest(listed),
                "ids": [{"stream": STREAM, "record_id": rid} for rid in listed],
            }
        },
    }


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R903.5")
@pytest.mark.asyncio
async def test_reconcile_receipt_reads_back() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        await _submit(client, _two_records(connector), "held")
        listed = ["item-1"]
        receipt = await _submit(client, _enumeration(connector, listed), "enumerate")
        assert receipt.reconcile is not None
        assert receipt.reconcile.ids_digest == _ids_digest(listed)
        assert receipt.reconcile.missing_count == 1
        # The sweep deletes later, after the grace window.
        assert receipt.reconcile.deleted_count == 0
        assert receipt.reconcile.override_principal == ""
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R903.3")
@pytest.mark.asyncio
async def test_enumeration_prunes_deleted_record() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        await _submit(client, _two_records(connector), "held")
        # The enumeration lists only `item-1`: `item-2` is planned for removal after the
        # grace window. The grace window cannot be waited out through the wire, so the
        # prune itself is proven with an injected clock in `sweep.rs`.
        receipt = await _submit(
            client, _enumeration(connector, ["item-1"]), "enumerate-prune"
        )
        assert receipt.reconcile is not None
        assert receipt.reconcile.missing_count == 1
        assert receipt.reconcile.deleted_count == 0
    finally:
        await _close(client, graph)
