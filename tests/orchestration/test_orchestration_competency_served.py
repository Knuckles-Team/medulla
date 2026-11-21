"""Served orchestration competency questions (MEDULLA-CQ-ORCH-R011.1, R011.4).

Q11 asks which tools a job needs loaded and how a connector is used. Both go
through the served ``Decide`` path; the fixture packs of
``fixtures/q11/packs/`` are imported through ``ConnectorPack.Import``.
"""

from __future__ import annotations

import asyncio
import dataclasses
import hashlib
import json
import os
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
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind, PackHeadRef

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


def _answer_params(tenant: str, kinds: list[str]) -> dict[str, Any]:
    return {
        "request": {
            "task_iri": "urn:task:xyz",
            "assembly": {
                "tenant_id": tenant,
                "policy": {"policy": "default"},
                "candidates": {"kinds": kinds},
                "requirements": {},
            },
        }
    }


_PARTS = ("steps", "staffing", "coverage", "guardrails", "bindings", "workflow", "dag")


@pytest.mark.spec("MEDULLA-CQ-ORCH-R008.5")
def test_composed_answer_has_seven_parts_with_provenance(clean_graph) -> None:
    from composed_support import (
        index_composed_repo,
        load_seed_into_request_graph,
        publish_composed_library,
    )

    suffix = f"-{uuid.uuid4().hex[:8]}"
    assert load_seed_into_request_graph(clean_graph)["triples"] > 0
    publish_composed_library(clean_graph, suffix)
    index_composed_repo(clean_graph, f"composed-repo-{suffix[1:]}")

    params = _answer_params(TEST_TENANT, ["tool"])
    first = clean_graph._send("AgentExecutionAnswer", params)
    second = clean_graph._send("AgentExecutionAnswer", params)
    assert first["task_iri"] == "urn:task:xyz"
    assert first == second, "the answer is stable over two calls"
    sealed = clean_graph._send("AgentAssemble", {"request": params["request"]["assembly"]})
    assert first["certificate_digest"] == sealed["record"]["record_digest"], (
        "certificate_digest is the digest of the sealed AgentAssemble record"
    )
    for name in _PARTS:
        part = first["parts"][name]
        assert part["evidence_class"], (name, part)
        if name == "coverage":
            # Only the rollout capability has a covering component; the audit
            # capability is the observable gap, so coverage may be its own
            # typed abstention. It is never a missing-source abstention.
            if part["status"] == "abstain":
                assert "ORCH_PART_" not in json.dumps(part["reason"]), part
                continue
        assert part["status"] == "answer", (name, part)
        assert part["provenance"], f"{name} carries at least one premise"


@pytest.mark.spec("MEDULLA-CQ-ORCH-R008.5")
def test_composed_answer_cross_tenant_abstains(clean_graph) -> None:
    from composed_support import load_seed_into_request_graph

    from conftest import tenant_request_context
    from medulla.client import SyncMedullaClient

    load_seed_into_request_graph(clean_graph)
    other = "tenant:composed-other"
    foreign = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=tenant_request_context(other),
    )
    try:
        answer = foreign._send("AgentExecutionAnswer", _answer_params(other, ["tool"]))
    finally:
        foreign.close()
    for name in _PARTS:
        assert answer["parts"][name]["status"] == "abstain", (name, answer["parts"][name])


@pytest.mark.spec("MEDULLA-CQ-ORCH-R008.3")
def test_missing_source_abstains_one_part(clean_graph) -> None:
    from composed_support import load_seed_into_request_graph, publish_tool_components

    # One tool above MAX_ASSEMBLY_CANDIDATES (64): the library scan refuses
    # CANDIDATE_SCOPE_TOO_LARGE for the assembly and for the coverage read.
    load_seed_into_request_graph(clean_graph)
    publish_tool_components(clean_graph, 65, prefix=f"tool-{uuid.uuid4().hex[:8]}")
    answer = clean_graph._send(
        "AgentExecutionAnswer", _answer_params(TEST_TENANT, ["tool"])
    )
    parts = answer["parts"]
    assert set(parts) == set(_PARTS), "all seven fields are present"

    def code(part: dict[str, Any]) -> str:
        assert part["status"] == "abstain", part
        return part["reason"]["code"]

    assert code(parts["coverage"]) == "ORCH_PART_NO_SOURCE"
    assert "CANDIDATE_SCOPE_TOO_LARGE" in parts["coverage"]["reason"]["detail"]
    assert "orch:part=coverage/ORCH_PART_NO_SOURCE" in json.dumps(parts["coverage"])
    for name in ("staffing", "bindings", "dag"):
        assert code(parts[name]) == "ORCH_PART_ASSEMBLY_ABSTAINED", name
    for name in ("steps", "workflow", "guardrails"):
        assert parts[name]["status"] == "answer", (name, parts[name])

_SEED = "https://knuckles-team.github.io/ontology/connector-seed#"


def _lexical(value: str | None) -> str:
    """A SPARQL cell as its bare text, without literal quotes or IRI brackets."""
    return (value or "").strip().strip('"').strip("<>")


@pytest.mark.spec("MEDULLA-CQ-ORCH-R011.5")
def test_q11_seed_changes() -> None:
    query = (
        "SELECT ?part ?outcome ?digest ?revision WHERE { "
        f"?r a <{_SEED}SeedImportReceipt> ; <{_SEED}part> ?part ; <{_SEED}outcome> ?outcome ; "
        f"<{_SEED}digest> ?digest ; <{_SEED}graphRevision> ?revision }} ORDER BY ?part"
    )

    async def receipt_rows(graph: str, want: dict[str, str]) -> dict[str, dict[str, str]]:
        """Poll the pack graph (the projection is asynchronous) until the outcomes match."""
        deadline = time.monotonic() + 120
        found: dict[str, dict[str, str]] = {}
        while time.monotonic() < deadline:
            client = await connect(graph)
            try:
                rows = await client.rdf.sparql(query)
                found = {
                    _lexical(row["part"]): {k: _lexical(v) for k, v in row.items()}
                    for row in rows
                }
            except Exception:  # the projection has not landed yet
                found = {}
            finally:
                await client.close()
            if {part: row["outcome"] for part, row in found.items()} == want:
                return found
            await asyncio.sleep(1)
        return found

    async def exercise() -> None:
        connector = f"seedchg-{uuid.uuid4().hex[:8]}"
        graph = _pack_graph(TEST_TENANT, connector)
        admin = await connect()
        try:
            first = await import_entries(admin, connector, _composed_seed())
            assert first["result"] == "imported", first
            head = PackHeadRef(
                binding_revision=first["receipt"]["binding_revision"],
                pack_digest=first["receipt"]["pack_digest"],
            )
            changed = [
                dataclasses.replace(
                    entry,
                    body=entry.body
                    + b"\n<urn:slot:xyz-c> <http://purl.org/dc/terms/source> "
                    b"<urn:source:topology-templates> .\n",
                )
                if entry.name == "catalog"
                else entry
                for entry in _composed_seed()
            ]
            second = await import_entries(admin, connector, changed, expected_head=head)
            assert second["result"] == "imported", second
        finally:
            await admin.close()

        rows = await receipt_rows(
            graph, {"catalog": "replaced", "identity": "unchanged", "references": "unchanged"}
        )
        assert {part: row["outcome"] for part, row in rows.items()} == {
            "catalog": "replaced",
            "identity": "unchanged",
            "references": "unchanged",
        }, rows
        revision = (
            "urn:medulla:seed:" + connector + ":" + second["receipt"]["seed"]["content_digest"]
        )
        for part, row in rows.items():
            assert row["digest"].startswith("sha256:"), (part, row)
            assert row["revision"] == revision, (part, row)

    asyncio.run(exercise())
