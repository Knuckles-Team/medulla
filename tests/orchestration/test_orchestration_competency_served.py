"""Served orchestration competency questions (MEDULLA-CQ-ORCH-R011.1, R011.4).

Q11 asks which tools a job needs loaded and how a connector is used. Both go
through the served ``Decide`` path; the fixture packs of
``fixtures/q11/packs/`` are imported through ``ConnectorPack.Import``.
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "decide"))

import pytest
from _commons_generation import (
    DIM,
    connect,
    establish_commons_generation,
    publish_schema,
)
from _decision_resident_fixture import publish
from conftest import TEST_TENANT
from ontology_pack_support import import_entries
from test_usage_guidance_served import STEPS, _guide, _import_pack
from test_usage_guidance_served import _schema_body

from medulla import ConnectorPackEntryContent
from medulla import decision_client as dc
from medulla.client import EngineResponseError
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

PACKS = Path(__file__).parent / "fixtures" / "q11" / "packs"
MAX_TOOLS = 32


def _fixture_packs(suffix: str) -> list[tuple[str, list[ConnectorPackEntryContent]]]:
    packs = []
    for path in sorted(PACKS.glob("*.json")):
        spec = json.loads(path.read_text(encoding="utf-8"))
        connector = f"{spec['connector']}-{suffix}"
        entries = [
            ConnectorPackEntryContent(
                kind=PackEntryKind.TOOL,
                uri=f"tool://{connector}/{tool['name']}",
                name=tool["name"],
                media_type="application/json",
                body=json.dumps(
                    {"name": tool["name"], "description": tool["description"]}
                ).encode(),
                input_schema=b'{"properties":{"q":{"type":"string"}},"type":"object"}',
                annotations=PackAnnotations(provides=[tool["provides"]]),
            )
            for tool in spec["tools"]
        ]
        packs.append((connector, entries))
    return packs


def _question(space: dict[str, Any], graph: str, required: list[str]) -> dict[str, Any]:
    return {
        "job_text": "look up a ticket and close it",
        "job_embedding": [1.0] + [0.0] * (DIM - 1),
        "required_capabilities": required,
        "max_tools": MAX_TOOLS,
        "max_agents": 4,
        "graph": graph,
        "embedding_version": space["space_digest"],
    }


@pytest.mark.spec("MEDULLA-CQ-ORCH-R011.1")
def test_q11_load_set() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            space = await establish_commons_generation(client)
            schema = await publish_schema(client)
        finally:
            await client.close()
        graph = f"q11-load-set-{uuid.uuid4().hex[:8]}"
        admin = await connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await connect(graph)
        decision = dc.DecisionClient(client=client)

        async def ask(required: list[str]) -> dict[str, Any]:
            request = dc.load_set_request(
                TEST_TENANT,
                f"q11-{uuid.uuid4().hex[:8]}",
                _question(space, graph, required),
                schema,
            )
            return (await decision.decide(request))["records"][0]

        try:
            for connector, entries in _fixture_packs(uuid.uuid4().hex[:8]):
                imported = await import_entries(client, connector, entries)
                assert imported["result"] == "imported", imported

            deadline = time.monotonic() + 240
            while True:
                try:
                    record = await ask([])
                except EngineResponseError as error:  # the feed has not indexed yet
                    assert time.monotonic() < deadline, error
                else:
                    answer = (record.get("answer") or {}).get("value") or {}
                    if answer.get("tools"):
                        break
                    assert time.monotonic() < deadline, record
                await asyncio.sleep(0.5)

            tools = answer["tools"]
            assert 0 < len(tools) <= MAX_TOOLS, tools
            facts = [premise["fact"] for premise in record["premises"]]
            for tool in tools:
                assert any(
                    fact.startswith(f"load_set:component={tool}/") for fact in facts
                ), (tool, facts)

            refused = await ask(["cap:q11-uncovered-capability"])
            assert refused["resolution_kind"] == "abstention", refused
            assert "LOAD_SET_NO_DESIGNATED_COMPONENT" in json.dumps(
                refused["outcome"]
            ), refused["outcome"]
            assert "cap:q11-uncovered-capability" in json.dumps(
                refused["outcome"]
            ), refused["outcome"]
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-CQ-ORCH-R011.4")
def test_q11_usage_guidance(clean_graph) -> None:
    suffix = uuid.uuid4().hex[:8]
    connector = f"q11-ug-{suffix}"
    graph = f"q11-usage-{suffix}"
    job = f"urn:job:{suffix}"
    schema = publish(
        clean_graph, f"q11-schema-{suffix}", "feature_schema", _schema_body()
    )
    asyncio.run(_import_pack(graph, connector, job))

    record = asyncio.run(_guide(schema, graph, connector, job))
    guidance = (record.get("answer") or {}).get("value") or {}
    tools = [step["tool"] for step in guidance["steps"]]
    assert sorted(tools) == sorted(
        f"mcp:{connector}/tool/{name}" for name in STEPS
    ), guidance
    # label has two predecessors, so it comes last.
    assert tools[-1] == f"mcp:{connector}/tool/label", guidance
    facts = [premise["fact"] for premise in record["premises"]]
    for name in STEPS:
        assert any(
            fact.startswith("guidance:triple=") and f"urn:step:{name}" in fact
            for fact in facts
        ), (name, facts)

    with pytest.raises(EngineResponseError) as unknown:
        asyncio.run(_guide(schema, graph, f"q11-unknown-{suffix}", job))
    assert "USAGE_GUIDANCE_UNKNOWN_CONNECTOR" in str(unknown.value)



@pytest.mark.spec("MEDULLA-CQ-ORCH-R008.2")
def test_envelope_assembled_by_graph_assemble() -> None:
    from medulla.generated import storage

    async def exercise() -> None:
        graph = f"q8-envelope-{uuid.uuid4().hex[:8]}"
        admin = await connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await connect(graph)
        try:
            for connector, entries in _fixture_packs(uuid.uuid4().hex[:8])[:1]:
                imported = await import_entries(client, connector, entries)
                assert imported["result"] == "imported", imported
            params = {
                "request": {
                    "task_iri": "urn:task:xyz",
                    "assembly": {
                        "tenant_id": TEST_TENANT,
                        "policy": {"policy": "default"},
                        "candidates": {"kinds": ["tool"]},
                        "requirements": {},
                    },
                }
            }
            opaque = await storage.send_agent_execution_answer(client, params, graph)
            answer = storage.decode_agent_execution_answer(opaque)
        finally:
            await client.close()
        assert answer.assembled_by == "graph.assemble", answer
        assert answer.task_iri == "urn:task:xyz"
        for part in (
            "steps",
            "staffing",
            "guardrails",
            "bindings",
            "workflow",
            "dag",
            "coverage",
        ):
            assert getattr(answer.parts, part) is not None, part

    asyncio.run(exercise())
