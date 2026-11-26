"""MEDULLA-SDLC-R016.1 and R016.2: unknowns trigger research; findings are cited.

L3: every call goes through the running engine. A research entry is an
``edit_plan_section`` proposal whose target starts with ``research:``; each
citation is a ``research_citation`` evidence whose record is an ingested source
node, and the entry text holds one ``Source: <record_id> (read at <ms>)`` line
per citation.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import time
import uuid
from pathlib import Path

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _decision_resident_fixture import publication
from conftest import (
    TEST_AGENT_ID,
    TEST_AUDIENCE,
    TEST_POLICY_VERSION,
    TEST_TENANT,
    request_context,
)

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.generated.index_repository import IndexRepositoryScope

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


async def _connect():
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _content_digest(properties: dict) -> str:
    from medulla.generated.digest import framed_sha256

    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return framed_sha256(b"eg/ingest-readback/node/v1", [compact.encode()])


async def _entry(client, project: str, statement: str, evidence: list[dict]) -> dict:
    proposal = {
        "project": project,
        "kind": "edit_plan_section",
        "target": "research:planunknown:demo:plan.md#L4",
        "statement": statement,
        "evidence": evidence,
    }
    return _body(await client._send("ProposalPut", {"proposal": proposal}, GRAPH))


# spec: MEDULLA-SDLC-R016.2
@pytest.mark.spec("MEDULLA-SDLC-R016.2")
@pytest.mark.asyncio
async def test_uncited_entry_is_refused(start_medulla_server) -> None:
    client = await _connect()
    try:
        suffix = uuid.uuid4().hex[:12]
        project = f"project-{suffix}"
        await client._send(
            "ProjectModeSet",
            {"request": {"project": project, "mode": "development"}},
            GRAPH,
        )
        source = f"source:research-{suffix}"
        properties = {"type": "WebSnapshot", "name": f"page-{suffix}"}
        await client._send(
            "AddNode",
            {
                "node_id": source,
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
            GRAPH,
        )
        citation = {
            "kind": "research_citation",
            "record_id": source,
            "digest": _content_digest(properties),
        }
        # No research_citation evidence at all.
        with pytest.raises(Exception, match="RESEARCH_ENTRY_UNCITED"):
            await _entry(client, project, "The finding.", [])
        # Evidence, but the text holds no Source line for it.
        with pytest.raises(Exception, match="RESEARCH_ENTRY_UNCITED"):
            await _entry(client, project, "The finding.", [citation])
        # A cited entry is accepted.
        statement = f"The finding.\nSource: {source} (read at 1700000000000)"
        receipt = await _entry(client, project, statement, [citation])
        assert receipt["evidence"] == 1
    finally:
        await client.close()


PLAN = (
    b"# Plan\n\n## Phase 0: Research\n"
    b"- Which parser handles nested fences?\n"
    b"- Settled and cited in `research.md` already\n\n"
    b"## Phase 1: Design\n- Not an unknown\n"
)
PLAN_PATH = "specs/demo/plan.md"


async def _ingest_plan(client, graph: str) -> None:
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": graph,
            "commits": [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": "f" * 40,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": PLAN_PATH,
                    "blob_digest": "sha256:" + hashlib.sha256(PLAN).hexdigest(),
                }
            ],
        }
    )
    await client.graph.index_repository([(PLAN_PATH, PLAN)], scope=scope, graph=graph)


def _item_key(unknown_id: str, text: str) -> str:
    digest = hashlib.sha256(text.encode()).hexdigest()
    return f"research-unknown:{unknown_id}:{digest}"


def _research_item(unknown_id: str, text: str, now_ms: int) -> dict:
    key = _item_key(unknown_id, text)
    return {
        "schema_version": "1",
        "context": {
            "schema_version": "2",
            "request_id": uuid.uuid4().hex,
            "subject_id": TEST_AGENT_ID,
            "tenant_id": TEST_TENANT,
            "agent_id": TEST_AGENT_ID,
            "scopes": [],
            "audience": TEST_AUDIENCE,
            "authentication_method": "local_process",
            "policy_version": TEST_POLICY_VERSION,
            "graph": "__commons__",
            "placement_epoch": None,
            "trace_id": uuid.uuid4().hex,
            "issued_at_ms": now_ms,
            "expires_at_ms": now_ms + 60_000,
        },
        "work_item_id": f"workitem:{key}",
        "idempotency_key": key,
        "command_digest": hashlib.sha256(key.encode()).hexdigest(),
        "kind": "research_web",
        "priority": 0,
        "depends_on": [],
        "input_ref": unknown_id,
        "policy_digest": "policy:research",
        "catalog_digest": "catalog:research",
        "model_digest": "model:research",
        "max_attempts": 1,
        "deadline_unix": None,
        "metadata": {"unknown_id": unknown_id},
        "provenance_refs": [],
        "max_tenant_in_flight": 8,
    }


async def _research_next(client, graph: str, unknowns: list[dict], budget: int) -> dict:
    schema = await client._send(
        "AgentComponent",
        publication(
            f"r016-1-schema-{uuid.uuid4().hex[:8]}",
            "feature_schema",
            ds.feature_schema_body([ds.feature("v", ds.number("v"), impute=0)]),
        ),
        graph,
    )
    component = _body(schema)["component"]
    pin = {
        "component_id": component["component_id"],
        "kind": "feature_schema",
        "definition_digest": component["definition_digest"],
    }
    options = [
        dc.DeclaredOption(
            row["unknown_id"],
            numbers={
                "cost": 250_000 << 32,
                "depth": 0,
                "gain": 1_000_000 << 32,
                "p_useful": 1 << 32,
            },
            texts={"subtask": row["unknown_id"]},
        )
        for row in unknowns
    ]
    request = dc.decide_request(
        TEST_TENANT,
        ("research.next", "research_next", "ordinary"),
        dc.declared(options),
        pin,
        params=[dc.param("budget_limit", "int", budget)],
    )
    decision = dc.DecisionClient(client=client, graph=graph)
    return (await decision.decide(request))["records"][0]


# spec: MEDULLA-SDLC-R016.1
@pytest.mark.spec("MEDULLA-SDLC-R016.1")
@pytest.mark.asyncio
async def test_only_development_and_evolving_codebases_trigger_research(
    start_medulla_server,
) -> None:
    client = await _connect()
    graph = f"research-loop-{uuid.uuid4().hex[:8]}"
    try:
        await client.tenants.create(graph, "Global")
        await _ingest_plan(client, graph)
        # A project with no mode record is a reference codebase: no research.
        with pytest.raises(Exception, match="MODE_REFERENCE_NO_PROPOSALS"):
            await client.spec_question("plan_unknowns", graph)
        await client._send(
            "ProjectModeSet",
            {"request": {"project": graph, "mode": "development"}},
            graph,
        )
        answer = await client.spec_question("plan_unknowns", graph)
        assert [(row["file"], row["line"], row["text"]) for row in answer["rows"]] == [
            (PLAN_PATH, "4", "Which parser handles nested fences?")
        ]
        unknown_id = f"planunknown:{graph}:{PLAN_PATH}#L4"
        assert answer["rows"][0]["unknown_id"] == unknown_id

        # One read fits the budget of 250,000 micro-units.
        record = await _research_next(client, graph, answer["rows"], 250_000)
        assert record["answer"]["type"] == "research"
        reads = record["answer"]["value"]["reads"]
        assert [read["source_id"] for read in reads] == [unknown_id]

        # The loop submits one work item per read; a second identical submit replays it.
        now_ms = int(time.time() * 1000)
        first = await client.work_items.submit(
            _research_item(unknown_id, answer["rows"][0]["text"], now_ms)
        )
        again = await client.work_items.submit(
            _research_item(unknown_id, answer["rows"][0]["text"], now_ms + 1)
        )
        assert again["work_item_id"] == first["work_item_id"]
        view = await client.work_items.get(
            tenant=TEST_TENANT, work_item_id=first["work_item_id"]
        )
        assert view is not None and view["kind"] == "research_web"
        assert view["input_ref"] == unknown_id
        assert view["metadata"] == {"unknown_id": unknown_id}
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R016.4
@pytest.mark.spec("MEDULLA-SDLC-R016.4")
@pytest.mark.asyncio
async def test_a_graph_with_no_research_contract_names_no_tool(
    start_medulla_server,
) -> None:
    """The loop's tools are read from the contracts the graph holds: a graph that holds
    none answers no tool, so a cited source that names one is refused."""
    client = await _connect()
    graph = f"research-tools-{uuid.uuid4().hex[:8]}"
    try:
        await client.tenants.create(graph, "Global")
        answer = await client.spec_question("research_tools", graph)
        assert answer["rows"] == []
        await client._send(
            "ProjectModeSet",
            {"request": {"project": graph, "mode": "development"}},
            graph,
        )
        source = f"source:research-{uuid.uuid4().hex[:8]}"
        properties = {
            "type": "WebSnapshot",
            "name": "page",
            "research_tool": "web_search",
        }
        await client._send(
            "AddNode",
            {
                "node_id": source,
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
            graph,
        )
        citation = {
            "kind": "research_citation",
            "record_id": source,
            "digest": _content_digest(properties),
        }
        statement = f"The finding.\nSource: {source} (read at 1700000000000)"
        with pytest.raises(Exception, match="RESEARCH_TOOL_UNKNOWN"):
            _body(
                await client._send(
                    "ProposalPut",
                    {
                        "proposal": {
                            "project": graph,
                            "kind": "edit_plan_section",
                            "target": f"research:planunknown:{graph}:plan.md#L4",
                            "statement": statement,
                            "evidence": [citation],
                        }
                    },
                    graph,
                )
            )
        await client.tenants.delete(graph)
    finally:
        await client.close()
