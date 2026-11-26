"""MEDULLA-CQ-ORCH-R703.7 served process sync and deploy.

L4: every call goes through the running engine over the wire client. The recorded
process definitions are served by a loopback ``FixtureSource`` and read through the
bounded host lifecycle (``host_read.completed_read``); ``source_sync`` takes the rows,
the invoice model is imported again as its next version, ``process_model_deploy``
queues the edited model, and the queue is approved and released through
``DecisionLog`` ``Writeback``.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "connector_competency"))

from conftest import TEST_TENANT, request_context
from fixture_source import FixtureSource
from host_read import completed_read
from test_contract_kinds_served import OPERATION

import medulla.decision_client as dc
from medulla.client import MedullaClient

GRAPH = "__commons__"
CONNECTION = "urn:eg:fixture:process-sync:process-engine"
CLASS_IRI = "https://knuckles-team.github.io/ontology/core#ProcessModel"
FIXTURES = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "tests"
    / "fixtures"
)
AUTHORITY = (FIXTURES / "process_sync" / "authority.ttl").read_text()
INVOICE = (FIXTURES / "process_equivalence" / "invoice-approval.bpmn").read_text()
APPROVAL_SPLIT = (FIXTURES / "process_equivalence" / "or_split_join.bpmn").read_text()
READ_LIMITS = {
    "max_rows": 10,
    "max_bytes": 1 << 20,
    "chunk_rows": 8,
    "chunk_bytes": 1 << 19,
    "deadline_ms": 10_000,
}


def _definition(key: str, document: str, revision: str) -> dict[str, Any]:
    return {
        "id": f"bpmn20:{key}:1",
        "system": "bpmn20",
        "model_key": key,
        "version": "1",
        "revision": revision,
        "document": document,
    }


def _plan_digest() -> str:
    return hashlib.sha256(CONNECTION.encode() + b"\0" + CLASS_IRI.encode()).hexdigest()


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )


async def _read_and_sync(
    client: MedullaClient,
    tmp: Path,
    run: str,
    definitions: list[dict[str, Any]],
    scope: str,
) -> dict[str, Any]:
    """Serve ``definitions`` from a loopback source, read them, and sync them."""
    recordings = tmp / run
    recordings.mkdir(parents=True)
    (recordings / f"{OPERATION}.json").write_text(json.dumps({"items": definitions}))
    fixture = FixtureSource("mcp_tool", recordings, operations=[OPERATION]).start()
    try:
        execution = await completed_read(
            client,
            kind="mcp_tool",
            connector=f"camunda-{run}",
            source_id=f"definitions-{run}",
            fixture=fixture,
            plan_digest=_plan_digest(),
            query_digest=_plan_digest(),
            limits=READ_LIMITS,
        )
    finally:
        fixture.close()
    return await client.graph.source_sync(CONNECTION, CLASS_IRI, execution, scope)


async def _journey(tmp: Path) -> dict[str, Any]:
    tag = uuid.uuid4().hex[:8]
    scope = f"urn:model:sync-{tag}"
    client = await _connect()
    try:
        await client._send(
            "GraphSchema",
            {
                "op": {
                    "op": "attach",
                    "source_id": f"admin:process-authority-{tag}",
                    "ontology_ttl": AUTHORITY,
                }
            },
            GRAPH,
            idempotency_key=f"attach-{tag}",
        )
        first = await _read_and_sync(
            client,
            tmp,
            f"{tag}-a",
            [
                _definition("invoice", INVOICE, "1"),
                _definition("split", APPROVAL_SPLIT, "1"),
            ],
            scope,
        )
        model_iri = f"{scope}:bpmn20:invoice"
        before = await client.graph.process_model_deploy(model_iri, CONNECTION)

        # Edit the invoice model: import its next version, then deploy it.
        edited = INVOICE.replace(
            'name="Check invoice"', 'name="Check invoice (edited)"'
        ).encode()
        await client.graph.process_document_import(
            {"format": "bpmn20", "document": edited}, model_iri, 1
        )
        deployed = await client.graph.process_model_deploy(model_iri, CONNECTION)

        decision = dc.DecisionClient(client=client)

        async def send(computation: dict[str, Any]) -> Any:
            return await decision.log(dc.writeback_op(TEST_TENANT, computation))

        queue_id = deployed["queue_id"]
        proposal_id = deployed["queued"][0]
        approved = await send(dc.writeback_approve(queue_id, proposal_id))
        released = await send(dc.writeback_release(queue_id))
        return {
            "first": first,
            "before": before,
            "deployed": deployed,
            "approved": approved,
            "released": released,
            "model_iri": model_iri,
        }
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R703.7")
def test_sync_then_deploy_releases_change_set(clean_graph, tmp_path):
    # spec: MEDULLA-CQ-ORCH-R703.7
    journey = asyncio.run(_journey(tmp_path))
    first = journey["first"]
    assert first["applied"] == 2 and first["proposed"] == 0, first
    before, deployed = journey["before"], journey["deployed"]
    assert deployed["proposed"] == 1 and len(deployed["queued"]) == 1
    # The edit changed the exported document, so the queue names a different proposal.
    assert (
        deployed["outbound"][0]["payload_digest"]
        != before["outbound"][0]["payload_digest"]
    )
    released = journey["released"]
    change_set = released.get("change_set", released)
    proposals = change_set["proposals"]
    assert len(proposals) == 1
    assert proposals[0]["external_id"] == journey["model_iri"]
    assert proposals[0]["id"] == deployed["queued"][0]
