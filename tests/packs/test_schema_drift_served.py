"""MEDULLA-TYPED-PACKS-R712 served acceptance: schema-drift containment end to end.

Drives the existing GraphSchema wire methods (repair render /
attach-approved / shadow validate) against the session engine:

* an approved baseline contract is attached only through a CONSUMED
  approval lease (the two-person approval path, DK R027);
* a drifted delta that is a BREAKING drift (a required field retyped and a
  required field missing) is quarantined, never applied: ``AttachApproved``
  refuses without a consumed approval lease, and ``ValidateRepair`` runs the
  shadow-graph check without attaching anything;
* only after the approval lease is consumed and a clean shadow ingest of the
  current data is stored (``ValidateRepair``, MEDULLA-DECISION-ENGINE-R102.2)
  does the drifted candidate activate — the unapproved candidate is never
  activated, and data that breaks the candidate keeps it out.
"""

# ruff: noqa: E501, I001

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path

import pytest
from pydantic import TypeAdapter
from conftest import request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "source_authority"))
from governed_ledger import (
    SOURCE_ID,
    Fields,
    attach_approved,
    candidate_digest,
    consume_approval_lease,
    issue_approval_lease,
    validate_repair,
)

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import reasoning
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind
from medulla.generated.graph_schema import (
    GraphSchemaSourcesView,
    SchemaSourceOriginViewApproved,
)
from medulla.generated.source_ingestion import (
    DriftAction,
    DriftClass,
    SourceIngestionOutcome,
    SourceIngestionReceipt,
)

# The approved baseline: two required fields.
BASELINE: dict[str, Fields] = {
    "fields": {
        "id": {"required": True, "types": ["integer"]},
        "name": {"required": True, "types": ["string"]},
    }
}
# The drifted delta: BREAKING — `name` is retyped (string -> integer, a set
# neither superset nor subset of the approved one) and the required `id`
# field no longer arrives.
DRIFTED: dict[str, Fields] = {
    "fields": {"name": {"required": True, "types": ["integer"]}}
}


# One record of the request graph per contract: conforming data the shadow
# ingest re-validates before the attach (MEDULLA-DECISION-ENGINE-R102.2).
RECORD_ID = "schema-drift-record"
# The class the candidate's shapes target; only its records are sampled.
RECORD_CLASS = "urn:eg:source:drift-served#Record"
BASELINE_RECORD = {"type": RECORD_CLASS, "id": 1, "name": "alpha"}
DRIFTED_RECORD = {"type": RECORD_CLASS, "name": 7}


async def approved_attach(
    client: MedullaClient,
    lease_id: str,
    contract: dict[str, Fields],
    record: dict[str, object],
) -> None:
    """Seed one conforming record, approve the candidate, store its clean shadow
    ingest report, then attach it."""
    await client.nodes.add(RECORD_ID, record)
    await issue_approval_lease(client, lease_id, candidate_digest(contract))
    await consume_approval_lease(client, lease_id)
    await validate_repair(client, contract["fields"])
    await attach_approved(client, lease_id, contract["fields"])


async def connect_isolated_graph() -> tuple[MedullaClient, str]:
    """A client on a fresh graph of its own, so the shadow ingest sees only the
    record the caller seeds."""
    graph = f"schema-drift-{uuid.uuid4().hex[:8]}"
    admin = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


async def close_isolated_graph(client: MedullaClient, graph: str) -> None:
    try:
        await client.tenants.delete(graph)
    finally:
        await client.close()


def only_source(listed: GraphSchemaSourcesView, expected_lease_id: str) -> bool:
    """Exactly one source sits under SOURCE_ID, attached by
    ``AttachApproved`` under ``expected_lease_id``."""
    sources = [
        source for source in listed.dynamic_sources if source.source_id == SOURCE_ID
    ]
    if len(sources) != 1:
        return False
    origin = sources[0].origin
    return isinstance(origin, SchemaSourceOriginViewApproved) and (
        origin.approval_lease_id == expected_lease_id
    )


# spec: MEDULLA-TYPED-PACKS-R083
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R712")
@pytest.mark.asyncio
async def test_breaking_drift_quarantines_and_activation_needs_approval() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        # 1. The baseline activates ONLY through a consumed approval lease.
        await client.graph.clear()
        await client.nodes.add(RECORD_ID, BASELINE_RECORD)
        await issue_approval_lease(
            client, "approval:baseline", candidate_digest(BASELINE)
        )
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(client, "approval:baseline", BASELINE["fields"])
        assert refused.value.code == "SCHEMA_APPROVAL_REQUIRED"
        await consume_approval_lease(client, "approval:baseline")
        # An approved candidate still needs a stored shadow ingest of the data.
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(client, "approval:baseline", BASELINE["fields"])
        assert refused.value.code == "SCHEMA_REPAIR_SHADOW_MISSING"
        await validate_repair(client, BASELINE["fields"])
        await attach_approved(client, "approval:baseline", BASELINE["fields"])
        baseline_listed = await reasoning.send_graph_schema_list(client, {})
        assert any(
            source.source_id == SOURCE_ID for source in baseline_listed.dynamic_sources
        ), "the approved baseline is attached"

        # 2. The drifted (breaking) delta is quarantined: no approval, no
        # lease at all -> refused, never applied.
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(client, "approval:absent", DRIFTED["fields"])
        assert refused.value.code == "SCHEMA_APPROVAL_REQUIRED"

        # 3. A lease that exists but is NOT consumed (still pending) is
        # equally a refusal.
        await issue_approval_lease(client, "approval:drift", candidate_digest(DRIFTED))
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(client, "approval:drift", DRIFTED["fields"])
        assert refused.value.code == "SCHEMA_APPROVAL_REQUIRED"
        assert "still pending" in (refused.value.detail or "")

        # 4. The shadow-graph validation renders and validates the drifted
        # candidate WITHOUT attaching it: changed is false and the source set
        # still holds only the baseline.
        assert not await validate_repair(client, DRIFTED["fields"])
        after_validate = await reasoning.send_graph_schema_list(client, {})
        assert only_source(after_validate, "approval:baseline"), (
            "the drifted delta was never applied: the baseline is still in force"
        )

        # 5. A consumed approval is not enough while the current data breaks
        # the drifted contract: the stored shadow report holds a violation.
        await consume_approval_lease(client, "approval:drift")
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(client, "approval:drift", DRIFTED["fields"])
        assert refused.value.code == "SCHEMA_REPAIR_SHADOW_VIOLATIONS"

        # 6. Once the data is repaired and re-validated, the drifted candidate
        # activates — replaced under the same key, lease recorded.
        await client.nodes.add(RECORD_ID, DRIFTED_RECORD)
        await validate_repair(client, DRIFTED["fields"])
        await attach_approved(client, "approval:drift", DRIFTED["fields"])
        final = await reasoning.send_graph_schema_list(client, {})
        assert only_source(final, "approval:drift"), "the drifted candidate activated"
    finally:
        await client.graph.clear()
        await client.close()


# --- MEDULLA-TYPED-PACKS-R714: the delivery path reports drift ---------------

DELIVERY_STREAM = "items"
DELIVERY_SOURCE_ID = f"approved:{DELIVERY_STREAM}"
DELIVERY_CLASS = f"urn:eg:source:{DELIVERY_STREAM}#Record"


def delivery_declaration(connector: str) -> ConnectorPackEntryContent:
    body = {
        "connector": connector,
        "schema_mappings": {
            "item": {
                "fields": {"id": "identifier", "name": "label"},
                "ontology_class": "Document",
            }
        },
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        # A resource declares its content schema (MCP resource contract).
        output_schema=b'{"type":"object"}',
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


def delivery_batch(
    connector: str, page: int, previous: int | None, payload: dict[str, object]
) -> dict[str, object]:
    return {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": False,
        "records": [
            {
                "stream": DELIVERY_STREAM,
                "record_id": f"item-{page}",
                "mapping_reference": f"connector:{connector}#mappings/item",
                "payload": payload,
                "updated_at": "2026-09-20T00:00:00Z",
                "provenance": {
                    "connector": connector,
                    "adapter_kind": "mcp",
                    "server": "demo-server",
                    "tool": "list_items",
                    "tool_schema_sha256": "07" * 32,
                    "source_uri": f"demo://items/item-{page}",
                },
            }
        ],
        "relationships": [],
        "withdrawals": [],
        "provider_checkpoint": {
            "stream": DELIVERY_STREAM,
            "position": {"page": page},
            "watermark": "2026-09-20T00:00:00Z",
        },
        "expected_previous_checkpoint": (
            None
            if previous is None
            # The accepted checkpoint exactly as committed, watermark included:
            # the cursor compare-and-set binds its whole digest.
            else {
                "stream": DELIVERY_STREAM,
                "position": {"page": previous},
                "watermark": "2026-09-20T00:00:00Z",
            }
        ),
    }


async def submit_delivery(
    client: MedullaClient, batch: dict[str, object]
) -> SourceIngestionReceipt:
    payload = await client._send(
        "SourceIngest",
        {"request": batch},
        idempotency_key=f"drift:{uuid.uuid4().hex}",
    )
    outcome = TypeAdapter(SourceIngestionOutcome).validate_python(payload)
    assert outcome.outcome == "accepted", outcome
    return outcome.receipt


# spec: MEDULLA-TYPED-PACKS-R710
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R714")
@pytest.mark.asyncio
async def test_delivery_reports_drift_class() -> None:
    connector = f"drift-{uuid.uuid4().hex[:10]}"
    admin = await connect()
    graph = f"drift-delivery-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(admin, connector, [delivery_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        # 1. No approved contract for the stream: the gate is skipped.
        skipped = await submit_delivery(
            client,
            delivery_batch(connector, 2, None, {"id": 1, "name": "alpha"}),
        )
        assert skipped.drift == []

        # 2. Approve the baseline contract for the stream.
        await client.nodes.add(
            "delivery-record", {"type": DELIVERY_CLASS, **{"id": 1, "name": "alpha"}}
        )
        lease = "approval:delivery"
        await issue_approval_lease(
            client,
            lease,
            candidate_digest(BASELINE, DELIVERY_SOURCE_ID),
            DELIVERY_SOURCE_ID,
        )
        await consume_approval_lease(client, lease)
        await validate_repair(client, BASELINE["fields"], DELIVERY_SOURCE_ID)
        await attach_approved(client, lease, BASELINE["fields"], DELIVERY_SOURCE_ID)

        # 3. A conforming delivery is compatible and accepted.
        conforming = await submit_delivery(
            client,
            delivery_batch(connector, 3, 2, {"id": 2, "name": "beta"}),
        )
        assert len(conforming.drift) == 1
        assert conforming.drift[0].stream == DELIVERY_STREAM
        assert conforming.drift[0].report.class_ is DriftClass.COMPATIBLE
        assert conforming.drift[0].report.action is DriftAction.ACCEPTED

    finally:
        try:
            await client.tenants.delete(graph)
        finally:
            await client.close()


async def _approved_delivery_client(connector: str, graph: str) -> MedullaClient:
    """A client on a graph whose delivery stream has an approved baseline contract."""
    admin = await connect()
    try:
        await import_entries(admin, connector, [delivery_declaration(connector)])
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    await client.nodes.add(
        "delivery-record", {"type": DELIVERY_CLASS, **{"id": 1, "name": "alpha"}}
    )
    lease = "approval:delivery"
    await issue_approval_lease(
        client,
        lease,
        candidate_digest(BASELINE, DELIVERY_SOURCE_ID),
        DELIVERY_SOURCE_ID,
    )
    await consume_approval_lease(client, lease)
    await validate_repair(client, BASELINE["fields"], DELIVERY_SOURCE_ID)
    await attach_approved(client, lease, BASELINE["fields"], DELIVERY_SOURCE_ID)
    return client


async def _count(client: MedullaClient, node_type: str) -> int:
    result = await client.query.uql(f"MATCH () WHERE type = '{node_type}'")
    return len(result["rows"])


# spec: MEDULLA-TYPED-PACKS-R712
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R715")
@pytest.mark.asyncio
async def test_breaking_delivery_is_quarantined() -> None:
    connector = f"drift-{uuid.uuid4().hex[:10]}"
    graph = f"drift-quarantine-{uuid.uuid4().hex[:8]}"
    client = await _approved_delivery_client(connector, graph)
    try:
        # The baseline delivery is accepted and advances the checkpoint to page 2.
        accepted = await submit_delivery(
            client, delivery_batch(connector, 2, None, {"id": 2, "name": "beta"})
        )
        assert accepted.drift[0].report.action is DriftAction.ACCEPTED
        records_before = await _count(client, DELIVERY_CLASS)

        # A drifted delivery (a required field retyped, another missing) is refused with its
        # report; no record is applied.
        with pytest.raises(
            EngineResponseError, match="SCHEMA_DRIFT_QUARANTINED"
        ) as refused:
            await submit_delivery(client, delivery_batch(connector, 3, 2, {"name": 7}))
        assert "breaking" in str(refused.value).lower()
        assert await _count(client, DELIVERY_CLASS) == records_before

        # One dead-letter item is parked, with digests only and no record body.
        quarantined = await client.query.uql("MATCH () WHERE type = 'SourceQuarantine'")
        assert len(quarantined["rows"]) == 1, quarantined
        stored = await client.nodes.properties(quarantined["rows"][0]["id"])
        assert stored is not None
        item = stored["dead_letter"]
        assert item["reason"] == "schema_drift"
        assert item["queue"] == "source-ingestion"
        assert item["payload_digest"].startswith("sha256:")
        assert "alpha" not in json.dumps(stored) and "beta" not in json.dumps(stored)

        # The checkpoint did not advance: the next delivery names page 2 as its base.
        resumed = await submit_delivery(
            client, delivery_batch(connector, 3, 2, {"id": 3, "name": "gamma"})
        )
        assert resumed.drift[0].report.action is DriftAction.ACCEPTED
    finally:
        try:
            await client.tenants.delete(graph)
        finally:
            await client.close()
