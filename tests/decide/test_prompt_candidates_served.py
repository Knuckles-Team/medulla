"""MEDULLA-DECISION-ENGINE-R814: connector system prompts as assembly candidates.

The canonical prompt blueprint is R814.8.

R814.3 to R814.7: two connector packs, each shipping one system prompt and one tool that
provides the prompt's capability, are imported through ``ConnectorPack.Import``. Served
``AgentAssemble`` narrows the one system-prompt slot to the prompt that covers the
requested capability, names that prompt, its connector, digest and covered capability in
the sealed record's premises, and the same question reads as one graph query.

R814.8: a packaged prompt must match the canonical blueprint, refused through the served
``ConnectorPack.import`` with a code that names the field path and never the field's
value."""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

import pytest
from _commons_generation import connect
from _sparql_solutions import solution_values
from conftest import TEST_TENANT, request_context
from ontology_pack_support import attach_pack, import_entries, ontology_entry
from test_connector_pack_planted import IMPORTER, _bind, _import

from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
    PackModelFacts,
    PackPromptRole,
)
from medulla.testing.synthetic.packs.content import canonical_json
from medulla.testing.synthetic.packs.malformed import base_spec, edited

LEGACY_SECRET = "legacy-body-text-that-must-not-be-echoed"  # sanitizer:ignore fixture


async def _import_prompt(connector: str, body: dict[str, Any]) -> Any:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        await _bind(client, connector, IMPORTER)
        pack = edited(base_spec(0, connector), "prompt", body=canonical_json(body))
        return await _import(client, pack, expected_head=None)
    finally:
        await client.close()


@pytest.mark.asyncio
async def test_non_canonical_prompt_is_refused_with_field_path() -> None:
    good = await _import_prompt(
        "blueprint-good",
        {
            "schema_version": "1.0",
            "task": "incident-triage",
            "type": "prompt",
            "source": "blueprint-good",
            "instructions": {"core_directive": "Triage the incident."},
        },
    )
    assert good["result"] == "imported", good

    legacy = await _import_prompt(
        "blueprint-legacy", {"messages": [], "content": LEGACY_SECRET}
    )
    assert legacy["result"] == "rejected", legacy
    violations = [
        v for v in legacy["violations"] if v["code"] == "PROMPT_BLUEPRINT_INVALID"
    ]
    assert violations, legacy
    detail = violations[0]["detail"]
    assert "content" in detail and "instructions.core_directive" in detail
    assert LEGACY_SECRET not in repr(legacy)


FIXTURES = Path(__file__).resolve().parents[1] / "fixtures"
CAPABILITY_A = "eg:capability/incident-triage"
CAPABILITY_B = "eg:capability/invoice-matching"
PROMPT_A = "mcp:prompt-pack-a/prompt/triage-system"
PROMPT_B = "mcp:prompt-pack-b/prompt/billing-system"
KINDS = {
    "prompt": PackEntryKind.PROMPT,
    "tool": PackEntryKind.TOOL,
}


def _entries(fixture: str) -> tuple[str, list[ConnectorPackEntryContent]]:
    pack = json.loads((FIXTURES / fixture).read_text())
    entries = []
    for entry in pack["entries"]:
        annotations = dict(entry["annotations"])
        role = annotations.pop("prompt_role", None)
        entries.append(
            ConnectorPackEntryContent(
                kind=KINDS[entry["kind"]],
                uri=entry["uri"],
                name=entry["name"],
                media_type=entry["media_type"],
                body=json.dumps(entry["body"], sort_keys=True).encode(),
                annotations=PackAnnotations(
                    **annotations,
                    prompt_role=PackPromptRole(role) if role else None,
                ),
            )
        )
    return pack["connector"], entries


def _base_pack() -> tuple[str, list[ConnectorPackEntryContent]]:
    """The model, skill and ontology a runnable agent needs."""
    connector = "prompt-base"

    def entry(kind: PackEntryKind, uri: str, name: str, **annotations: Any):
        return ConnectorPackEntryContent(
            kind=kind,
            uri=uri,
            name=name,
            media_type="application/json",
            body=json.dumps({"name": name}).encode(),
            annotations=PackAnnotations(**annotations),
        )

    model = PackModelFacts(
        provider="provider",
        model_identity="model",
        context_window_tokens=128_000,
        max_output_tokens=4_096,
        supports_tools=True,
        supports_structured_output=True,
        supports_vision=False,
    )
    return connector, [
        entry(
            PackEntryKind.MODEL_PROFILE,
            f"model-profile://{connector}/model",
            "model",
            model=model,
        ),
        entry(PackEntryKind.SKILL, f"skill://{connector}/skill", "skill"),
        ontology_entry(
            connector,
            "agent",
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<https://example.org/{connector}/Thing> a owl:Class .\n",
        ),
    ]


async def _graph_with_both_packs() -> Any:
    suffix = uuid.uuid4().hex[:8]
    graph = f"prompt-candidates-{suffix}"
    admin = await connect()
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await connect(graph)
    packs = [_base_pack()] + [
        _entries(fixture) for fixture in ("prompt_pack_a.json", "prompt_pack_b.json")
    ]
    for connector, entries in packs:
        imported = await import_entries(client, connector, entries)
        assert imported["result"] == "imported", imported
        await attach_pack(client, connector)
    return client


def _request(capability: str) -> dict[str, Any]:
    """The assembly request for one required capability over every candidate kind."""
    return {
        "tenant_id": TEST_TENANT,
        "requirements": {"capabilities": [capability]},
        "candidates": {
            "kinds": ["model_profile", "system_prompt", "tool", "skill", "ontology"],
            "classification_under": None,
        },
        "policy": {"policy": "default"},
    }


async def _assemble(client: Any, capability: str) -> dict[str, Any]:
    return await client._send("AgentAssemble", {"request": _request(capability)})


# spec: MEDULLA-DECISION-ENGINE-R814.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R814.3")
@pytest.mark.asyncio
async def test_assemble_reads_candidates_from_imported_pack() -> None:
    client = await _graph_with_both_packs()
    try:
        answer = await _assemble(client, CAPABILITY_A)
        agent = answer["agents"][0]
        assert agent["system_prompt"]["component_id"] == PROMPT_A, agent
        other = await _assemble(client, CAPABILITY_B)
        assert other["agents"][0]["system_prompt"]["component_id"] == PROMPT_B
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R814.5
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R814.5")
@pytest.mark.asyncio
async def test_record_names_prompt_connector_digest_and_capability() -> None:
    client = await _graph_with_both_packs()
    try:
        answer = await _assemble(client, CAPABILITY_A)
        facts = {premise["fact"] for premise in answer["record"]["premises"]}
        assert f"prompt_component_id={PROMPT_A}" in facts, facts
        assert "connector=prompt-pack-a" in facts, facts
        assert f"covered_capability={CAPABILITY_A}" in facts, facts
        assert any(fact.startswith("digest=") for fact in facts), facts
        assert any(fact.startswith("prompt_score_q32=") for fact in facts), facts
        assert not any("prompt-pack-b" in fact for fact in facts), facts
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R814.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R814.6")
@pytest.mark.asyncio
async def test_dynamic_agent_gets_the_matching_connector_system_prompt() -> None:
    client = await _graph_with_both_packs()
    try:
        for capability, expected in (
            (CAPABILITY_A, PROMPT_A),
            (CAPABILITY_B, PROMPT_B),
        ):
            answer = await _assemble(client, capability)
            assert answer["record"]["outcome"]["outcome"] == "solved", answer["record"]
            assert answer["agents"][0]["system_prompt"]["component_id"] == expected
    finally:
        await client.close()


SPARQL = (
    "PREFIX : <https://knuckles-team.github.io/ontology/core#> "
    "PREFIX cp: <https://knuckles-team.github.io/ontology/connector-pack#> "
    "SELECT ?prompt ?connector ?digest WHERE { "
    "?prompt a :PromptCandidate ; :systemPromptFor ?t ; cp:ofConnector ?connector ; "
    "cp:digest ?digest . ?t :requiresCapability ?need . "
    "<urn:task:incident-triage> :requiresCapability ?need }"
)
UQL = (
    "MATCH (p:PromptCandidate)-[:systemPromptFor]->(t:Task)-[:requiresCapability]->(c)"
    "<-[:requiresCapability]-(y:Task) |> WHERE y.id = $task "
    "|> RETURN p.id, p.connector, p.digest"
)


# spec: MEDULLA-DECISION-ENGINE-R814.7
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R814.7")
@pytest.mark.asyncio
async def test_system_prompt_query_lists_candidates_for_a_task() -> None:
    client = await _graph_with_both_packs()
    try:
        sparql_rows = solution_values(await client.rdf.sparql(SPARQL))
        uql = await client.query.uql(UQL, {"task": "urn:task:incident-triage"})
        sparql_set = {
            (row["prompt"], row["connector"], row["digest"]) for row in sparql_rows
        }
        uql_set = {
            (row["p.id"], row["p.connector"], row["p.digest"]) for row in uql["rows"]
        }
        assert sparql_set == uql_set
        assert {prompt for prompt, _, _ in sparql_set} == {PROMPT_A}, sparql_set
    finally:
        await client.close()
