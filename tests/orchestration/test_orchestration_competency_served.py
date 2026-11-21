"""Served orchestration competency questions (MEDULLA-CQ-ORCH-R011.1, R011.4).

Q11 asks which tools a job needs loaded and how a connector is used. Both go
through the served ``Decide`` path; the fixture packs of
``fixtures/q11/packs/`` are imported through ``ConnectorPack.Import``.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import struct
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


COMPOSED = Path(__file__).parent / "fixtures" / "composed"
_CORE = "https://knuckles-team.github.io/ontology/core#"
_SWARM = "https://knuckles-team.github.io/ontology/swarm#"
_DC = "http://purl.org/dc/terms/"


def _pack_graph(tenant: str, connector: str) -> str:
    """The physical pack graph name: ``Digest256::framed`` over tenant and connector."""
    domain = b"eg/connector-pack-projection-graph/v1"
    hasher = hashlib.sha256()
    hasher.update(b"eg/framed-sha256/v1\0")
    hasher.update(struct.pack(">I", len(domain)))
    hasher.update(domain)
    fields = [tenant.encode(), connector.encode()]
    hasher.update(struct.pack(">I", len(fields)))
    for field in fields:
        hasher.update(struct.pack(">Q", len(field)))
        hasher.update(field)
    return f"pack__{hasher.hexdigest()}"


def _composed_seed() -> list[ConnectorPackEntryContent]:
    parts = []
    for key in ("identity", "catalog", "references"):
        path = COMPOSED / f"seed_{key}.ttl"
        assert path.is_file(), f"missing composed fixture input {path.name}"
        body = path.read_bytes()
        parts.append(
            ConnectorPackEntryContent(
                kind=PackEntryKind.SEED_PART,
                uri=f"seed://{key}",
                name=key,
                media_type="text/turtle",
                body=body,
                annotations=PackAnnotations(),
            )
        )
    return parts


def _select(query: str):
    async def run(client: Any) -> list[dict[str, str | None]]:
        return await client.rdf.sparql(query)

    return run


@pytest.mark.spec("MEDULLA-CQ-ORCH-R008.4")
def test_fixture_ingest_produces_expected_part_rows() -> None:
    templates = json.loads((COMPOSED / "templates.json").read_text(encoding="utf-8"))
    assert len(templates["templates"]) == 3, "three Agent Library templates"
    repo = sorted(
        path.relative_to(COMPOSED / "repo").as_posix()
        for path in (COMPOSED / "repo").rglob("*")
        if path.is_file()
    )
    assert repo == ["README.md", "billing/parse.py", "billing/render.py"]
    for key in ("identity", "catalog", "references"):
        triples = [
            line
            for line in (COMPOSED / f"seed_{key}.ttl").read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.startswith(("@prefix", "#"))
        ]
        assert len(triples) < 5000

    async def exercise() -> None:
        connector = f"composed-{uuid.uuid4().hex[:8]}"
        admin = await connect()
        try:
            imported = await import_entries(admin, connector, _composed_seed())
        finally:
            await admin.close()
        assert imported["result"] == "imported", imported
        assert set(imported["receipt"]["seed"]["outcomes"].values()) == {"inserted"}

        graph = _pack_graph(TEST_TENANT, connector)
        queries = {
            "steps": (
                f"SELECT ?step WHERE {{ <urn:task:xyz> <{_CORE}hasStep> ?step }} ORDER BY ?step",
                4,
            ),
            "workflow": (
                f"SELECT ?from ?to WHERE {{ ?from <{_CORE}handsOffTo> ?to }} ORDER BY ?from",
                2,
            ),
            "dag": (
                f"SELECT ?from ?to WHERE {{ ?from <{_SWARM}feeds> ?to }} ORDER BY ?from",
                2,
            ),
            "staffing": (
                f"SELECT ?slot WHERE {{ <urn:template:xyz> <{_SWARM}hasSlot> ?slot }} ORDER BY ?slot",
                3,
            ),
            "coverage": (
                f"SELECT ?capability WHERE {{ <urn:task:xyz> <{_CORE}requiresCapability> ?capability }} "
                "ORDER BY ?capability",
                2,
            ),
            "guardrails": (
                f"SELECT ?rule WHERE {{ ?rule a <{_CORE}BestPractice> }} ORDER BY ?rule",
                1,
            ),
        }
        deadline = time.monotonic() + 120
        rows: dict[str, list[dict[str, str | None]]] = {}
        while time.monotonic() < deadline:
            client = await connect(graph)
            try:
                for name, (query, _expected) in queries.items():
                    rows[name] = await client.rdf.sparql(query)
            except Exception:  # the projection has not landed yet
                rows = {}
            finally:
                await client.close()
            if rows and all(len(rows[n]) == want for n, (_q, want) in queries.items()):
                break
            await asyncio.sleep(1)
        for name, (_query, expected) in queries.items():
            assert len(rows.get(name, [])) == expected, (name, rows.get(name))

    asyncio.run(exercise())
