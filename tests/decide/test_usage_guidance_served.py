"""MEDULLA-DECISION-ENGINE-R177.6: connector usage guidance over the wire.

One connector pack is imported through ``ConnectorPack.Import`` (three tools
and the surface links that say which tool realises which capability). The job
class's steps (``:hasStep``, ``:precedes``, ``:needsSkill``) are written to the
pack's graph through the served SPARQL update path. Two steps (find, read) may
run in either order; both precede label. Outcomes for the two tools are
recorded through ``DecisionLog``, then the served ``Decide`` ``usage_guidance``
question returns the order whose tool has the higher success rate, with a
citation for every step. ``Decide`` writes nothing and adds no method.
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

import pytest
from _commons_generation import connect
from _decision_resident_fixture import publish
from conftest import TEST_TENANT
from ontology_pack_support import attach_pack, import_entries, ontology_entry

from decide.test_path_success_served import (
    _publish_policy,
    _schema_body,
    _seed,
)
from medulla import ConnectorPackEntryContent
from medulla import decision_client as dc
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

CP = "https://knuckles-team.github.io/ontology/connector-pack#"
CORE = "https://knuckles-team.github.io/ontology/core#"
SPARQL_UPDATE_EVENT = "sparql_http_update_v1"
STEPS = ("find", "read", "label")


def _tool(connector: str, name: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps({"name": name, "description": f"{name} a note"}).encode(),
        input_schema=b'{"properties":{"q":{"type":"string"}},"type":"object"}',
        annotations=PackAnnotations(),
    )


def _surface(connector: str) -> ConnectorPackEntryContent:
    feature = f"urn:feature:{connector}"
    turtle = f"@prefix cp: <{CP}> .\n"
    turtle += f"<mcp-server://{connector}> cp:providesFeature <{feature}> .\n"
    for name in STEPS:
        turtle += f"<{feature}> cp:realizedBy <tool://{connector}/{name}> .\n"
        turtle += f"<tool://{connector}/{name}> cp:providesCapability <cap:{name}> .\n"
    return ontology_entry(connector, "surface", turtle)


def _steps_update(job: str) -> str:
    triples = []
    for name in STEPS:
        triples.append(f"<{job}> <{CORE}hasStep> <urn:step:{name}> .")
        triples.append(f"<urn:step:{name}> <{CORE}needsSkill> <skill://{name}> .")
        triples.append(f"<skill://{name}> <{CP}providesCapability> <cap:{name}> .")
    for name in ("find", "read"):
        triples.append(f"<urn:step:{name}> <{CORE}precedes> <urn:step:label> .")
    triples.append(
        "<skill://label> <http://www.w3.org/2000/01/rdf-schema#comment> "
        '"never label a note the reporter can see" .'
    )
    return "INSERT DATA { " + " ".join(triples) + " }"


async def _wait_projected(client, connector: str) -> None:
    query = (
        f"PREFIX cp: <{CP}> SELECT ?f WHERE "
        f"{{ <mcp-server://{connector}> cp:providesFeature ?f }}"
    )
    deadline = time.monotonic() + 180
    while not await client.rdf.sparql(query):
        assert time.monotonic() < deadline, "the pack surface was not projected"
        await asyncio.sleep(0.5)


async def _import_pack(graph: str, connector: str, job: str) -> None:
    admin = await connect()
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await connect(graph)
    try:
        entries = [_tool(connector, name) for name in STEPS] + [_surface(connector)]
        imported = await import_entries(client, connector, entries)
        assert imported["result"] == "imported", imported
        await attach_pack(client, connector)
        await _wait_projected(client, connector)
        await client.apply_mutation(SPARQL_UPDATE_EVENT, _steps_update(job))
    finally:
        await client.close()


async def _guide(schema, graph: str, connector: str, job: str) -> dict:
    client = await connect(graph)
    try:
        request = dc.usage_guidance_request(
            TEST_TENANT,
            f"usage-{uuid.uuid4().hex[:8]}",
            f"mcp-server://{connector}",
            job,
            schema,
            graph=graph,
        )
        return (await dc.DecisionClient(client=client).decide(request))["records"][0]
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R177.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R177.6")
def test_how_to_use_connector_answers_from_graph(clean_graph) -> None:
    suffix = uuid.uuid4().hex[:8]
    connector = f"ug-{suffix}"
    graph = f"usage-guidance-{suffix}"
    job = f"urn:job:{suffix}"
    tool = {name: f"mcp:{connector}/tool/{name}" for name in STEPS}
    schema = publish(
        clean_graph, f"r177-6-schema-{suffix}", "feature_schema", _schema_body()
    )
    policy = {
        "policy": "pinned",
        "component": _publish_policy(clean_graph, f"r177-6-policy-{suffix}"),
    }
    asyncio.run(_import_pack(graph, connector, job))

    # The two legal orders of the swappable steps: with no outcomes the lower
    # component id (find) leads; the higher success rate (read) must win.
    asyncio.run(_seed(schema, policy, tool["find"], [True] * 2 + [False] * 8))
    asyncio.run(_seed(schema, policy, tool["read"], [True] * 9 + [False]))

    record = asyncio.run(_guide(schema, graph, connector, job))
    guidance = (record.get("answer") or {}).get("value") or {}
    ordered = [step["tool"] for step in guidance["steps"]]
    assert ordered == [tool["read"], tool["find"], tool["label"]], guidance
    facts = [premise["fact"] for premise in record["premises"]]
    assert f"guidance:outcomes={tool['read']}/9/10" in facts, facts
    assert f"guidance:outcomes={tool['find']}/2/10" in facts, facts
    for name in STEPS:
        assert any(
            fact.startswith("guidance:triple=") and f"urn:step:{name}" in fact
            for fact in facts
        ), (name, facts)
    assert any(
        "never label a note the reporter can see" in c for c in guidance["cautions"]
    ) or any(fact.startswith("declared:") for fact in facts), guidance
