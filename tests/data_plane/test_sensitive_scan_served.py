"""Served sensitive-column scan (MEDULLA-TYPED-PACKS-R803).

A `DatasetProfile` job scans the sampled values of each profiled column with the
built-in detectors and the tenant's own `SensitiveDetector` nodes, and proposes
one `ClassificationClaim` per passing detector. A claim records the detector,
sample size, hit ratio and scan time and never a matched value. A detector that
fails validation fails the job before any scan and writes no claim.
"""

# ruff: noqa: E501

from __future__ import annotations

import json
import os
import time
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

CARD = "4111111111111111"


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _finish(client: Any, job_id: str) -> tuple[str, dict]:
    status: dict = {}
    for _ in range(300):
        status = client.jobs.status(job_id)
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            return name, status
        time.sleep(0.1)
    raise AssertionError(f"profile job did not finish: {status!r}")


def _claims(client: Any, graph: str) -> list[dict]:
    rows = _decode(
        client._send(
            "GetNodesByLabel", {"label": "ClassificationClaim", "limit": 100}, graph
        )
    )
    return [_decode(blob) for _node_id, blob in rows]


def _table(client: Any, graph: str) -> str:
    table = f"scan_{uuid.uuid4().hex[:8]}"
    client.query.sql(
        f"CREATE TABLE {table} (id int primary key, card text, email text)"
    )
    rows = ", ".join(f"({n}, '{CARD}', 'person{n}@example.test')" for n in range(1, 21))
    client.query.sql(f"INSERT INTO {table} VALUES {rows}")
    return table


# spec: MEDULLA-TYPED-PACKS-R803.6
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R803")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R803.3")
def test_profile_job_proposes_claim_without_values(start_medulla_server) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R803.3: one proposed claim per passing detector."""
    client = _client()
    graph = f"scan-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        table = _table(client, graph)
        job = client.jobs.submit(
            graph, {"DatasetProfile": {"dataset": table, "columns": ["card", "email"]}}
        )
        name, _ = _finish(client, job["job_id"])
        assert name == "Succeeded"
        claims = {claim["asset"]: claim for claim in _claims(client, graph)}
        assert set(claims) == {f"col:{table}.card", f"col:{table}.email"}
        assert claims[f"col:{table}.card"]["categories"] == ["payment_card"]
        assert claims[f"col:{table}.email"]["categories"] == ["contact_email"]
        for claim in claims.values():
            assert claim["state"] == "proposed"
            assert claim["tenant_id"] == TEST_TENANT
            assert CARD not in str(claim) and "example.test" not in str(claim)
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R803.4")
def test_detector_and_promotion_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R803.4: an invalid detector fails the job, no claim."""
    client = _client()
    graph = f"scan-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        table = _table(client, graph)
        bad = {
            "type": "SensitiveDetector",
            "id": "detector:bad",
            "category": "custom",
            "method": "pattern",
            "rule": {"kind": "pattern", "regex": "x"},
            "threshold": 2.0,
            "tenant_id": TEST_TENANT,
        }
        client._send(
            "AddNode",
            {
                "node_id": "detector:bad",
                "properties_msgpack": msgpack.packb(bad, use_bin_type=True),
            },
            graph,
        )
        job = client.jobs.submit(
            graph, {"DatasetProfile": {"dataset": table, "columns": ["card"]}}
        )
        name, status = _finish(client, job["job_id"])
        assert name == "Failed"
        assert "DETECTOR_THRESHOLD_INVALID" in str(status)
        assert _claims(client, graph) == []
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R803.5")
def test_claim_evidence_reads_back(start_medulla_server) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R803.5: the claim carries its detector evidence."""
    client = _client()
    graph = f"scan-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        table = _table(client, graph)
        job = client.jobs.submit(
            graph, {"DatasetProfile": {"dataset": table, "columns": ["card"]}}
        )
        name, _ = _finish(client, job["job_id"])
        assert name == "Succeeded"
        [claim] = _claims(client, graph)
        record = json.loads(claim["record"])
        provenance = record["claim"]["provenance"]
        assert provenance["kind"] == "detector"
        assert provenance["detector_id"] == "detector:card_number"
        assert provenance["sample_size"] == 20
        assert provenance["hit_ratio"] == 1.0
        assert provenance["scanned_at_ms"] > 0
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R803.7")
def test_enrich_proposes_claim_for_sensitive_document(start_medulla_server) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R803.7: card numbers in chunks yield one proposed claim."""
    client = _client()
    graph = f"scan-doc-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        # Card numbers written as one token and as four tokens, in several chunks.
        paragraph = (
            "The customer paid with card 4111 1111 1111 1111 on the first order "
            "and with card 4111111111111111 on the second order. "
        )
        text = "# Orders\n\n" + (paragraph * 6 + "\n\n") * 4
        document_id = f"doc:cards-{uuid.uuid4().hex[:8]}"
        client._send(
            "AddNode",
            {
                "node_id": document_id,
                "properties_msgpack": msgpack.packb(
                    {
                        "type": "Document",
                        "title": "Orders",
                        "text": text,
                        "url": "https://example.invalid/orders",
                    },
                    use_bin_type=True,
                ),
            },
            graph,
        )
        receipt = _decode(
            client._send(
                "EnrichDocument",
                {
                    "request": {
                        "document_id": document_id,
                        "max_chunk_bytes": 600,
                        "overlap_bytes": 80,
                    }
                },
                graph,
            )
        )
        assert len(receipt["chunk_ids"]) > 1

        # One proposed claim for the payment-card detector, naming the Document, no raw value.
        claims = [
            claim for claim in _claims(client, graph) if claim["asset"] == document_id
        ]
        assert [claim["categories"] for claim in claims] == [["payment_card"]]
        claim = claims[0]
        assert claim["state"] == "proposed"
        assert claim["tenant_id"] == TEST_TENANT
        assert CARD not in str(claim) and "4111 1111" not in str(claim)

        # A document with no card number yields no claim.
        clean_id = f"doc:clean-{uuid.uuid4().hex[:8]}"
        client._send(
            "AddNode",
            {
                "node_id": clean_id,
                "properties_msgpack": msgpack.packb(
                    {
                        "type": "Document",
                        "title": "Plain",
                        "text": "# Plain\n\nNothing sensitive is written here. " * 20,
                        "url": "https://example.invalid/plain",
                    },
                    use_bin_type=True,
                ),
            },
            graph,
        )
        client._send(
            "EnrichDocument",
            {"request": {"document_id": clean_id, "max_chunk_bytes": 600}},
            graph,
        )
        assert not [c for c in _claims(client, graph) if c["asset"] == clean_id]
    finally:
        client.tenants.delete(graph)
        client.close()
