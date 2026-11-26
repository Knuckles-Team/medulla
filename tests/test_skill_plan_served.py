"""Served skill documents: MEDULLA-REPO-INGEST-R002.1.3.

A `skill_documents` batch compiles a SKILL.md into a `SkillCompilation` node, linked
to its directory node and carrying a `revision`. A `Decide` task plan pinned to an
older document digest is refused as stale.
"""

from __future__ import annotations

import hashlib
import json
import os
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT, request_context
from ingest.test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _ingest_client,
)
from pydantic import TypeAdapter

from medulla.client import EngineResponseError, MedullaClient, SyncMedullaClient
from medulla.decision_client import param, task_plan_request
from medulla.generated.source_ingestion import (
    SourceIngestionDisposition,
    SourceIngestionOutcome,
)

DIRECTORY = "skills/inspect"
STEPS = [
    ("read", "Read the node", "GetNodeProperties", []),
    ("report", "Report it", None, ["read"]),
]


def _document(report_text: str) -> str:
    lines = ["# Inspect", "", "## Steps"]
    for step_id, instruction, capability, after in STEPS:
        text = report_text if step_id == "report" else instruction
        lines.append(f"- {step_id}: {text}")
        if capability:
            lines.append(f"  capability: {capability}")
        if after:
            lines.append(f"  after: {', '.join(after)}")
    return "\n".join(lines) + "\n"


def _proposal(report_text: str) -> dict[str, Any]:
    tasks = []
    for step_id, instruction, capability, after in STEPS:
        text = report_text if step_id == "report" else instruction
        task: dict[str, Any] = {"task_id": step_id, "prompt": text, "depends_on": after}
        if capability:
            task["assignee"] = capability
        tasks.append(task)
    return {
        "tasks": tasks,
        "output_task_id": STEPS[-1][0],
        "max_concurrency": len(STEPS),
        "max_steps": len(STEPS),
    }


def _batch(connector: str, document: str, page: int) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "skill_documents",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {
            "skill_documents": [{"directory_ref": DIRECTORY, "text": document}]
        },
        "provider_checkpoint": {"stream": STREAM, "position": {"skills": page}},
        "expected_previous_checkpoint": (
            None if page == 1 else {"stream": STREAM, "position": {"skills": page - 1}}
        ),
    }


async def _ingest(client: MedullaClient, batch: dict[str, Any]) -> None:
    payload = await client._send(
        "SourceIngest",
        {"request": json.loads(json.dumps(batch))},
        idempotency_key=f"skills-{uuid.uuid4().hex}",
    )
    outcome = TypeAdapter(SourceIngestionOutcome).validate_python(payload)
    assert outcome.outcome == "accepted", outcome
    assert outcome.receipt.disposition is SourceIngestionDisposition.COMMITTED, outcome


async def _compilation(client: MedullaClient) -> tuple[str, dict[str, Any]]:
    nodes = await client.nodes.list_by_label("SkillCompilation", 0)
    assert len(nodes) == 1, nodes
    node_id = nodes[0][0]
    return node_id, await client.nodes.properties(node_id) or {}


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.1.3")
@pytest.mark.asyncio
async def test_changed_document_marks_binding_stale() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    sync = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        first_text = _document("Report it")
        await _ingest(client, _batch(connector, first_text, 1))
        node_id, first = await _compilation(client)
        assert first["revision"] == 1
        assert (
            first["document_digest"] == hashlib.sha256(first_text.encode()).hexdigest()
        )
        # The graph holds the digests and the directory reference, never the text.
        assert "Read the node" not in json.dumps(first)

        changed_text = _document("Report the result")
        await _ingest(client, _batch(connector, changed_text, 2))
        node_id_again, second = await _compilation(client)
        assert node_id_again == node_id
        assert second["revision"] == 2
        assert second["document_digest"] != first["document_digest"]

        pin = publish(
            sync,
            "skill-binding-schema-" + uuid.uuid4().hex,
            "feature_schema",
            schema_body(),
        )

        def ask(document_digest: str, report_text: str) -> dict[str, Any]:
            request = task_plan_request(
                TEST_TENANT, "skill-binding", _proposal(report_text), pin
            )
            request["params"] = sorted(
                [
                    *request["params"],
                    param(
                        "skill_binding",
                        "skill_binding",
                        {"node_id": node_id, "document_digest": document_digest},
                    ),
                    param("graph", "text", graph),
                ],
                key=lambda entry: entry["name"],
            )
            return request

        with pytest.raises(EngineResponseError, match="SKILL_BINDING_STALE") as stale:
            sync._send(
                "Decide",
                {"request": ask(first["document_digest"], "Report the result")},
            )
        assert "document_digest differs" in str(stale.value)
        fresh = sync._send(
            "Decide",
            {"request": ask(second["document_digest"], "Report the result")},
        )
        assert fresh["records"], fresh
    finally:
        sync.close()
        await _close(client, graph)


def _raw_document(*step_lines: str) -> str:
    return "# Inspect\n\n## Steps\n" + "\n".join(step_lines) + "\n"


REFUSED_DOCUMENTS = {
    "SKILL_STEP_ID_MISSING": _raw_document("- : no id here"),
    "SKILL_STEP_ID_DUPLICATE": _raw_document("- a: one", "- a: two"),
    "SKILL_DEPENDENCY_UNRESOLVED": _raw_document("- a: one", "  after: ghost"),
    "SKILL_CYCLE": _raw_document("- a: one", "  after: b", "- b: two", "  after: a"),
    "SKILL_CAPABILITY_UNRESOLVED": _raw_document(
        "- a: one", "  capability: NoSuchCapability"
    ),
    "SKILL_TYPE_UNSUPPORTED": _raw_document("- a: one", "  colour: red"),
    "SKILL_SPAN_MISSING": "# Inspect\n\nNo steps section at all.\n",
}


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.1.4")
@pytest.mark.asyncio
async def test_skill_plan_refusals() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        for page, (code, text) in enumerate(REFUSED_DOCUMENTS.items(), start=1):
            with pytest.raises(EngineResponseError, match=code):
                await client._send(
                    "SourceIngest",
                    {"request": _batch(connector, text, page)},
                    idempotency_key=f"skill-refusal-{code}-{uuid.uuid4().hex}",
                )
        # A refusal writes nothing: no compilation node exists after all seven.
        assert await client.nodes.list_by_label("SkillCompilation", 0) == []
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.1.5")
@pytest.mark.asyncio
async def test_skill_compilation_reads_back_without_document_copy() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    second = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        text = _document("Report it")
        await _ingest(client, _batch(connector, text, 1))
        node_id, written = await _compilation(client)
        read = await second.nodes.properties(node_id) or {}
        assert read == written
        for field in (
            "document_digest",
            "plan_digest",
            "capability_digest",
            "type_digest",
        ):
            assert len(read[field]) == 64, field
        assert read["directory_ref"] == DIRECTORY
        # No node of the graph holds the document text.
        for label in ("SkillCompilation", "SkillDirectory"):
            for other_id, _ in await second.nodes.list_by_label(label, 0):
                body = json.dumps(await second.nodes.properties(other_id))
                assert "Read the node" not in body, other_id
                assert "# Inspect" not in body, other_id
    finally:
        await second.close()
        await _close(client, graph)
