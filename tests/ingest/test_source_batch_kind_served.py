"""Served `SourceIngest` batch kind: MEDULLA-INGEST-WORKER-R902.3.1."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated.source_ingestion import (
    SourceBatchKind,
    SourceIngestionRequest,
)

CHECKPOINT = {"stream": "items", "position": {"page": 1}}
BASE = {
    "connector": "demo-connector",
    "mode": "delta",
    "strict_schema": True,
    "records": [],
    "provider_checkpoint": CHECKPOINT,
}


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R902.3.1")
def test_mismatched_kind_is_refused(clean_graph):
    # The kind/extension rule is enforced by the typed request decoder the
    # engine and the client share, so the request is refused before it is
    # sent: the client names it as an invalid engine request with the code.
    mismatched = {**BASE, "kind": "access"}
    with pytest.raises(
        ValueError,
        match="is not a valid engine request: .*SOURCE_BATCH_KIND_MISMATCH",
    ):
        clean_graph._send("SourceIngest", {"request": mismatched})


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R902.3.1")
def test_client_model_carries_kind_into_the_digest():
    records = SourceIngestionRequest.model_validate(BASE)
    access = SourceIngestionRequest.model_validate(
        {**BASE, "kind": "access", "extension": {"access": []}}
    )
    assert records.kind is SourceBatchKind.RECORDS
    assert records.canonical_digest() != access.canonical_digest()
    with pytest.raises(ValidationError):
        SourceIngestionRequest.model_validate(
            {**BASE, "kind": "access", "extension": {"access": [], "groups": []}}
        )
