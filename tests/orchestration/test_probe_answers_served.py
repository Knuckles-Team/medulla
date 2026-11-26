"""Engine-side proof of the six orchestration probes (MEDULLA-CQ-ORCH-R009.1 to R009.6).

Each test drives the engine path an Arbora intent tool reaches, without Arbora. The
companion L5 cases in the Arbora repository assert the same answers through the
intent tools. Every answer lists what it read: node ids, source records or
component references.

* probe 1: ``NlQuery`` plans a UQL group-by over node labels and runs it.
* probe 2: ``Decide`` ``usage_guidance`` for a connector and a job class.
* probe 3: the Agent Library, read from the catalog nodes its publish emits.
* probe 4: ``Discover`` over ingested source documents.
* probe 5: ``Decide`` ``load_set`` for the job text ``list docker containers``.
* probe 6: ``Decide`` ``load_set`` for the job text ``list running containers``; the
  call arguments are built by Arbora from the tool's input schema, never by the
  engine.
"""

# ruff: noqa: E501

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
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "retrieval"))

import pytest
from _commons_generation import (
    DIM,
    connect,
    establish_commons_generation,
    publish_schema,
)
from _decision_resident_fixture import publish
from _sparql_solutions import solution_values
from conftest import TEST_TENANT
from expansion_harness import ChatStub, served_engine
from expansion_harness import connect as sync_connect
from ontology_pack_support import attach_pack, import_entries, ontology_entry
from test_catalog_assets_served import (  # noqa: F401  (catalog_graph is a fixture)
    CATALOG,
    RDF_TYPE,
    SOURCE_RECORD,
    _connect,
    _publish_agent,
    catalog_graph,
)
from test_usage_guidance_served import (
    CORE,
    CP,
    SPARQL_UPDATE_EVENT,
    _schema_body,
    _tool,
    _wait_projected,
)

from medulla import ConnectorPackEntryContent
from medulla import decision_client as dc
from medulla.client import EngineResponseError
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

PACKS = Path(__file__).parent / "fixtures" / "probes" / "packs"
CONTAINER_STEPS = ("pull_image", "create_container", "start_container")


def _spec_marker(row: str) -> pytest.MarkDecorator:
    return pytest.mark.spec(f"MEDULLA-CQ-ORCH-{row}")


# --------------------------------------------------------------------------- probe 1

NODE_COUNT_UQL = "MATCH () |> GROUP BY label AGGREGATE COUNT AS count |> LIMIT 100000"
LABELS = {"ProbeHost": 5, "ProbeService": 4, "ProbeVolume": 3}


@pytest.fixture(scope="module")
def planner_engine(tmp_path_factory):
    """An engine whose NL planner is a scripted endpoint answering the group-by."""
    stub = ChatStub(None, raw=NODE_COUNT_UQL)
    with served_engine(tmp_path_factory, stub) as sock:
        yield sock, stub
    stub.close()


@_spec_marker("R009.1")
def test_probe_1(planner_engine) -> None:
    """How many nodes does the graph hold, by label?"""
    sock, stub = planner_engine
    client = sync_connect(sock)
    graph = f"probe-1-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        # An empty graph answers zero rows, never an invented count.
        assert client.query.nl_query("how many nodes by label", graph=graph) == []

        total = 0
        for label, count in LABELS.items():
            for index in range(count):
                client.nodes.add(f"{graph}:{label}:{index}", {"type": label})
                total += 1
        assert total == 12

        rows = client.query.nl_query("how many nodes by label", graph=graph)
        assert stub.calls >= 2, "the planner is asked once per question"
        by_label = {row["id"]: row.get("channels", row).get("count") for row in rows}
        assert {label: int(by_label[label]) for label in LABELS} == LABELS, rows
        # The group-by total equals the engine's own node count.
        assert sum(int(count) for count in by_label.values()) == client.nodes.count()
    finally:
        client.tenants.delete(graph)
        client.close()


# --------------------------------------------------------------------------- probe 2


def _container_surface(connector: str) -> ConnectorPackEntryContent:
    feature = f"urn:feature:{connector}"
    turtle = f"@prefix cp: <{CP}> .\n"
    turtle += f"<mcp-server://{connector}> cp:providesFeature <{feature}> .\n"
    for name in CONTAINER_STEPS:
        turtle += f"<{feature}> cp:realizedBy <tool://{connector}/{name}> .\n"
        turtle += f"<tool://{connector}/{name}> cp:providesCapability <cap:{name}> .\n"
    return ontology_entry(connector, "surface", turtle)


def _container_steps_update(job: str) -> str:
    triples = []
    for name in CONTAINER_STEPS:
        triples.append(f"<{job}> <{CORE}hasStep> <urn:step:{name}> .")
        triples.append(f"<urn:step:{name}> <{CORE}needsSkill> <skill://{name}> .")
        triples.append(f"<skill://{name}> <{CP}providesCapability> <cap:{name}> .")
    triples.append(
        f"<urn:step:pull_image> <{CORE}precedes> <urn:step:create_container> ."
    )
    triples.append(
        f"<urn:step:create_container> <{CORE}precedes> <urn:step:start_container> ."
    )
    triples.append(
        "<skill://start_container> <http://www.w3.org/2000/01/rdf-schema#comment> "
        '"never start a container whose image was not pulled" .'
    )
    return "INSERT DATA { " + " ".join(triples) + " }"


async def _import_container_pack(graph: str, connector: str, job: str) -> None:
    admin = await connect()
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await connect(graph)
    try:
        entries = [_tool(connector, name) for name in CONTAINER_STEPS]
        entries.append(_container_surface(connector))
        imported = await import_entries(client, connector, entries)
        assert imported["result"] == "imported", imported
        await attach_pack(client, connector)
        await _wait_projected(client, connector)
        await client.apply_mutation(SPARQL_UPDATE_EVENT, _container_steps_update(job))
    finally:
        await client.close()


async def _usage_guidance(schema, graph: str, connector: str, job: str) -> dict:
    client = await connect(graph)
    try:
        request = dc.usage_guidance_request(
            TEST_TENANT,
            f"probe-2-{uuid.uuid4().hex[:8]}",
            f"mcp-server://{connector}",
            job,
            schema,
            graph=graph,
        )
        return (await dc.DecisionClient(client=client).decide(request))["records"][0]
    finally:
        await client.close()


@_spec_marker("R009.2")
def test_probe_2(clean_graph) -> None:
    """How can I deploy a container with container-manager?"""
    suffix = uuid.uuid4().hex[:8]
    connector = f"probe-container-manager-{suffix}"
    graph = f"probe-2-{suffix}"
    job = f"urn:job:deploy_container:{suffix}"
    schema = publish(
        clean_graph, f"probe-2-schema-{suffix}", "feature_schema", _schema_body()
    )

    # No pack is imported yet: the connector is unknown, and nothing is invented.
    admin_graph = f"probe-2-empty-{suffix}"

    async def unknown() -> None:
        admin = await connect()
        try:
            await admin.tenants.create(admin_graph)
        finally:
            await admin.close()
        await _usage_guidance(schema, admin_graph, connector, job)

    with pytest.raises(EngineResponseError) as refused:
        asyncio.run(unknown())
    assert "USAGE_GUIDANCE_UNKNOWN_CONNECTOR" in str(refused.value)

    asyncio.run(_import_container_pack(graph, connector, job))
    record = asyncio.run(_usage_guidance(schema, graph, connector, job))
    guidance = (record.get("answer") or {}).get("value") or {}
    ordered = [step["tool"] for step in guidance["steps"]]
    assert ordered == [f"mcp:{connector}/tool/{name}" for name in CONTAINER_STEPS], (
        guidance
    )
    assert len(ordered) >= 3
    facts = [premise["fact"] for premise in record["premises"]]
    for name in CONTAINER_STEPS:
        assert any(
            fact.startswith("guidance:triple=") and f"urn:step:{name}" in fact
            for fact in facts
        ), (name, facts)
    assert any(
        "never start a container whose image was not pulled" in caution
        for caution in guidance["cautions"]
    ) or any(fact.startswith("declared:") for fact in facts), guidance


# --------------------------------------------------------------------------- probe 3


def _agent_versions(client) -> dict[str, str]:
    """The Agent Library's published revisions in the client's graph, by source record."""
    rows = solution_values(
        client.rdf.sparql(
            f"SELECT ?n ?s WHERE {{ ?n <{RDF_TYPE}> <{CATALOG}AgentVersion> ; "
            f"<{SOURCE_RECORD}> ?s }}"
        )
    )
    return {row["s"]: row["n"] for row in rows}


@_spec_marker("R009.3")
def test_probe_3(catalog_graph) -> None:  # noqa: F811
    """Which agents are in the Agent Library?"""
    graph_a, admin_a = catalog_graph
    suffix = uuid.uuid4().hex[:8]

    # An empty library answers an empty list, not an error.
    assert _agent_versions(admin_a) == {}

    graph_b = f"medulla-probe-3-b-{suffix}"
    admin_a.tenants.create(graph_b, graph_type="Global")
    admin_b = _connect(graph_b, None)
    try:
        mine = [f"agent:probe-3-a-{index}-{suffix}" for index in range(3)]
        theirs = f"agent:probe-3-b-{suffix}"
        for agent_id in mine:
            _publish_agent(admin_a, graph_a, agent_id, 0, "1.0.0")
        _publish_agent(admin_b, graph_b, theirs, 0, "1.0.0")

        listed = _agent_versions(admin_a)
        # Exactly the three published agents, each as its revision's catalog node
        # with the source record that names it; the other graph's agent is absent.
        assert sorted(listed) == sorted(
            f"agent-library:{agent_id}:1" for agent_id in mine
        )
        assert all(
            node.startswith("urn:medulla:asset:agent-version:")
            for node in listed.values()
        )
        assert not any(theirs in record for record in listed)
        assert list(_agent_versions(admin_b)) == [f"agent-library:{theirs}:1"]
    finally:
        admin_b.close()
        admin_a.tenants.delete(graph_b)


# --------------------------------------------------------------------------- probe 4

SPEC_PAGES = {
    "doc:arbora-operations-spec": (
        "Arbora operations specification",
        "Arbora serves one intent surface: the intent tools find, ask, why, write, "
        "act and manage replace a catalogue of per operation tools, so a client "
        "discovers an operation by intent and the planner stays small.",
    ),
    "doc:ecosystem-boundaries": (
        "Ecosystem boundaries",
        "The intent surface is the one door: find, ask, why, write, act and manage "
        "route every request to its owner, and no repository grows a second door.",
    ),
}


@_spec_marker("R009.4")
def test_probe_4(clean_graph) -> None:
    """Why does Arbora use the intent surface?"""
    for node_id, (title, text) in SPEC_PAGES.items():
        clean_graph.nodes.add(
            node_id,
            {
                "type": "SourceDocument",
                "name": title,
                "description": text,
                "digest": "sha256:" + uuid.uuid5(uuid.NAMESPACE_URL, node_id).hex * 2,
            },
        )

    found = clean_graph.graph.discover(
        ["intent", "surface", "arbora"],
        [],
        k=5,
        query_text="why does arbora use the intent surface",
    )
    hits = found["hits"]
    cited = {hit["id"] for hit in hits}
    assert set(SPEC_PAGES) <= cited, hits
    explanation = " ".join(hit.get("description") or "" for hit in hits)
    for tool in ("find", "ask", "why", "write", "act", "manage"):
        assert tool in explanation, (tool, explanation)

    # No document matches: the answer has no hit to cite, and the verdict stands in
    # for the abstention. The engine never invents a reason.
    none = clean_graph.graph.discover(
        ["zzzunmatchable", "qqqnothing"],
        [],
        k=5,
        query_text="zzzunmatchable qqqnothing",
    )
    assert not [hit for hit in none["hits"] if hit["id"] in SPEC_PAGES], none
    assert "verdict" in none


# --------------------------------------------------------------------------- probes 5, 6

MAX_TOOLS = 8


def _probe_packs(suffix: str) -> list[tuple[str, list[ConnectorPackEntryContent]]]:
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
                input_schema=b'{"properties":{"all":{"type":"boolean"}},"type":"object"}',
                annotations=PackAnnotations(provides=[tool["provides"]]),
            )
            for tool in spec["tools"]
        ]
        packs.append((connector, entries))
    return packs


async def _load_set(job_text: str, required: list[str]) -> tuple[dict[str, Any], str]:
    client = await connect()
    try:
        space = await establish_commons_generation(client)
        schema = await publish_schema(client)
    finally:
        await client.close()
    graph = f"probe-load-set-{uuid.uuid4().hex[:8]}"
    admin = await connect()
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await connect(graph)
    decision = dc.DecisionClient(client=client)
    suffix = uuid.uuid4().hex[:8]

    async def ask(needed: list[str]) -> dict[str, Any]:
        request = dc.load_set_request(
            TEST_TENANT,
            f"probe-{uuid.uuid4().hex[:8]}",
            {
                "job_text": job_text,
                "job_embedding": [1.0] + [0.0] * (DIM - 1),
                "required_capabilities": needed,
                "max_tools": MAX_TOOLS,
                "max_agents": 4,
                "graph": graph,
                "embedding_version": space["space_digest"],
            },
            schema,
        )
        return (await decision.decide(request))["records"][0]

    try:
        for connector, entries in _probe_packs(suffix):
            imported = await import_entries(client, connector, entries)
            assert imported["result"] == "imported", imported
        deadline = time.monotonic() + 240
        while True:
            try:
                record = await ask(required)
            except EngineResponseError as error:  # the feed has not indexed yet
                assert time.monotonic() < deadline, error
            else:
                answer = (record.get("answer") or {}).get("value") or {}
                if answer.get("tools"):
                    return record, suffix
                assert time.monotonic() < deadline, record
            await asyncio.sleep(0.5)
    finally:
        await client.close()


@_spec_marker("R009.5")
def test_probe_5() -> None:
    """Find a tool that lists Docker containers."""
    record, suffix = asyncio.run(_load_set("list docker containers", []))
    answer = (record.get("answer") or {}).get("value") or {}
    tools = [json.dumps(tool) for tool in answer["tools"]]
    assert "list_containers" in tools[0], tools
    assert not any("send_email" in tool for tool in tools), tools
    facts = [premise["fact"] for premise in record["premises"]]
    for tool in answer["tools"]:
        assert any(fact.startswith(f"load_set:component={tool}/") for fact in facts), (
            tool,
            facts,
        )


@_spec_marker("R009.5")
def test_probe_5_no_matching_capability() -> None:
    """No tool covers the capability: the load set refuses by name."""

    async def exercise() -> dict[str, Any]:
        client = await connect()
        try:
            space = await establish_commons_generation(client)
            schema = await publish_schema(client)
        finally:
            await client.close()
        graph = f"probe-5-refusal-{uuid.uuid4().hex[:8]}"
        admin = await connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await connect(graph)
        try:
            request = dc.load_set_request(
                TEST_TENANT,
                f"probe-5-{uuid.uuid4().hex[:8]}",
                {
                    "job_text": "list docker containers",
                    "job_embedding": [1.0] + [0.0] * (DIM - 1),
                    "required_capabilities": ["cap:probe-uncovered"],
                    "max_tools": MAX_TOOLS,
                    "max_agents": 4,
                    "graph": graph,
                    "embedding_version": space["space_digest"],
                },
                schema,
            )
            return (await dc.DecisionClient(client=client).decide(request))["records"][
                0
            ]
        finally:
            await client.close()

    refused = asyncio.run(exercise())
    assert refused["resolution_kind"] == "abstention", refused
    assert "LOAD_SET_NO_DESIGNATED_COMPONENT" in json.dumps(refused["outcome"])


@_spec_marker("R009.6")
def test_probe_6() -> None:
    """Delegate: list running containers."""
    record, _ = asyncio.run(_load_set("list running containers", []))
    answer = (record.get("answer") or {}).get("value") or {}
    tools = [json.dumps(tool) for tool in answer["tools"]]
    # The engine names the tool; Arbora builds the call arguments from the tool's
    # input schema and holds the preview, so no call has run here.
    assert "list_containers" in tools[0], tools
    assert not any("send_email" in tool for tool in tools), tools
    facts = [premise["fact"] for premise in record["premises"]]
    assert any(fact.startswith("load_set:component=") for fact in facts), facts
    assert record["resolution_kind"] != "abstention", record
