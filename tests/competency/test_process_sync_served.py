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
from host_read import HostFetchRefused, completed_read
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
ARIS_CONNECTION = (
    "https://knuckles-team.github.io/ontology/source-record/process-sync#aris"
)
ARIS_AUTHORITY = (FIXTURES / "process_sync" / "aris_authority.ttl").read_text()
ARIS_DEPLOY_OPERATION = "canonical_to_aml"
ARIS_NOT_CONFIGURED = "ARIS_NOT_CONFIGURED"
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


def _plan_digest(connection: str = CONNECTION) -> str:
    return hashlib.sha256(connection.encode() + b"\0" + CLASS_IRI.encode()).hexdigest()


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
    connection: str = CONNECTION,
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
            plan_digest=_plan_digest(connection),
            query_digest=_plan_digest(connection),
            limits=READ_LIMITS,
        )
    finally:
        fixture.close()
    return await client.graph.source_sync(connection, CLASS_IRI, execution, scope)


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


# MEDULLA-CQ-ORCH-R703.8: the ARIS repository as a process source.

EQUIVALENCE = FIXTURES / "process_equivalence"


def _aris_definition(key: str, canonical_file: str, revision: str) -> dict[str, Any]:
    """One ``aris-mcp`` fixture repository model, as its canonical form."""
    return {
        "id": f"aris_epc:{key}:1",
        "system": "aris_epc",
        "model_key": key,
        "version": "1",
        "revision": revision,
        "canonical": json.loads((EQUIVALENCE / canonical_file).read_text()),
    }


async def _attach_aris(client: MedullaClient, tag: str) -> None:
    await client._send(
        "GraphSchema",
        {
            "op": {
                "op": "attach",
                "source_id": f"admin:aris-authority-{tag}",
                "ontology_ttl": ARIS_AUTHORITY,
            }
        },
        GRAPH,
        idempotency_key=f"attach-aris-{tag}",
    )


async def _aris_journey(tmp: Path) -> dict[str, Any]:
    tag = uuid.uuid4().hex[:8]
    scope = f"urn:model:aris-{tag}"
    client = await _connect()
    try:
        await _attach_aris(client, tag)
        invoice = _aris_definition(
            "invoice", "invoice-approval.epc.canonical.json", "1"
        )
        split = _aris_definition("split", "or_split_join.epc.canonical.json", "1")
        first = await _read_and_sync(
            client, tmp, f"{tag}-a", [invoice, split], scope, ARIS_CONNECTION
        )

        # The graph side: rename the invoice model's "Check invoice" task.
        model_node = f"processmodel:{scope}:aris_epc:invoice:v1"
        model = await client.nodes.properties(model_node)
        assert model is not None, model_node
        element_ids = json.loads(model["elementIds"])
        edit: dict[str, Any] | None = None
        for element_id in element_ids:
            properties = await client.nodes.properties(element_id) or {}
            for name, value in properties.items():
                if value == "Check invoice":
                    edit = {
                        "op": "upsert_node",
                        "id": element_id,
                        "properties": {name: "Check (graph)"},
                    }
        assert edit is not None, "the named task node"
        await client.lifecycle.batch_update([edit])

        # The source side: the split model's next recording under the same key.
        changed_split = _aris_definition(
            "split", "decision_step.epc.canonical.json", "2"
        )
        second = await _read_and_sync(
            client, tmp, f"{tag}-b", [invoice, changed_split], scope, ARIS_CONNECTION
        )
        repeat = await _read_and_sync(
            client, tmp, f"{tag}-c", [invoice, changed_split], scope, ARIS_CONNECTION
        )

        decision = dc.DecisionClient(client=client)

        async def send(computation: dict[str, Any]) -> Any:
            return await decision.log(dc.writeback_op(TEST_TENANT, computation))

        queue_id = second["queue_id"]
        await send(dc.writeback_approve(queue_id, second["queued"][0]))
        released = await send(dc.writeback_release(queue_id))
        return {
            "first": first,
            "second": second,
            "repeat": repeat,
            "released": released,
        }
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R703.8")
def test_aris_repository_sync_then_deploy(clean_graph, tmp_path):
    # spec: MEDULLA-CQ-ORCH-R703.8
    journey = asyncio.run(_aris_journey(tmp_path))
    first, second = journey["first"], journey["second"]
    assert first["applied"] == 2 and first["proposed"] == 0, first
    # One source change applied, one graph change proposed outbound.
    assert second["applied"] == 1 and second["proposed"] == 1, second
    assert len(second["outbound"]) == 1 and len(second["queued"]) == 1, second
    draft = second["outbound"][0]
    assert draft["record_key"] == "aris_epc:invoice:1"
    assert len(draft["payload_digest"]) == 64 and int(draft["payload_digest"], 16) >= 0
    assert draft["connector_operation"] == ARIS_DEPLOY_OPERATION
    # The same graph state proposes the same canonical payload again.
    assert journey["repeat"]["outbound"][0]["payload_digest"] == draft["payload_digest"]
    released = journey["released"]
    change_set = released.get("change_set", released)
    proposals = change_set["proposals"]
    assert len(proposals) == 1
    assert proposals[0]["id"] == second["queued"][0]
    assert proposals[0]["external_id"] == "aris_epc:invoice:1"
    assert proposals[0]["connector_operation"] == ARIS_DEPLOY_OPERATION


async def _unconfigured_journey(tmp: Path) -> dict[str, Any]:
    tag = uuid.uuid4().hex[:8]
    scope = f"urn:model:aris-off-{tag}"
    client = await _connect()
    try:
        await _attach_aris(client, tag)
        recordings = tmp / f"off-{tag}"
        recordings.mkdir(parents=True)
        (recordings / f"{OPERATION}.json").write_text(json.dumps({"items": []}))
        fixture = FixtureSource("mcp_tool", recordings, operations=[OPERATION]).start()
        # Not configured: the source is down and the host's fetch is refused.
        fixture.stop()
        refused: HostFetchRefused | None = None
        try:
            await completed_read(
                client,
                kind="mcp_tool",
                connector=f"aris-off-{tag}",
                source_id=f"definitions-off-{tag}",
                fixture=fixture,
                plan_digest=_plan_digest(ARIS_CONNECTION),
                query_digest=_plan_digest(ARIS_CONNECTION),
                limits=READ_LIMITS,
                unavailable_code=ARIS_NOT_CONFIGURED,
            )
        except HostFetchRefused as error:
            refused = error
        finally:
            fixture.close()
        assert refused is not None, "the fetch must be refused"
        # The read never completed: a sync over it is refused and takes no row.
        sync_error: str | None = None
        try:
            await client.graph.source_sync(
                ARIS_CONNECTION, CLASS_IRI, refused.execution, scope
            )
        except Exception as error:  # the engine's refusal is the answer under test
            sync_error = str(error)
        models = [
            node_id
            for node_id, _ in await client.nodes.list_by_label("ProcessModel")
            if scope in node_id
        ]
        return {"refused": refused, "sync_error": sync_error, "models": models}
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R703.8")
def test_unconfigured_aris_source_is_unavailable(clean_graph, tmp_path):
    # spec: MEDULLA-CQ-ORCH-R703.8
    result = asyncio.run(_unconfigured_journey(tmp_path))
    refused = result["refused"]
    assert refused.refusal == "SOURCE_UNAVAILABLE"
    assert refused.code == ARIS_NOT_CONFIGURED
    # Never an empty success: the sync is refused, nothing is written.
    assert result["sync_error"], "a sync over an unfinished read must be refused"
    assert result["models"] == []
