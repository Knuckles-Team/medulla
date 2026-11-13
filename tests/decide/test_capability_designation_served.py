"""MEDULLA-DECISION-ENGINE-R172.11: capability designation over the wire.

The entry point is the served ``Decide`` method with a
``capability_designation`` question. The commons embedding generation is
established through the served generation cycle (the first generation
bootstraps); an agent, two tools and a skill are published through the served
Agent Library; the engine's capability feed embeds the tools and the skill
into the resident index; and the question observes subsumption-aware
(a tool that declares a subclass of the required capability), policy-tag
filtered designations with their eligibility. Outcomes recorded through
``DecisionLog`` op ``capability_outcome`` re-rank the designations, and the
same rewards are observed again after the engine restarts on its durable
store.
"""

from __future__ import annotations

import asyncio
import sys
import time
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

import pytest
from _commons_generation import (
    Engine,
    connect,
    establish_commons_generation,
    publish_schema,
    restartable_engine,
    vector,
)
from _decision_resident_fixture import publication
from conftest import TEST_TENANT
from ontology_pack_support import attach_pack, import_entries, ontology_entry

from medulla import decision_client as dc
from medulla import decision_stat as ds

ONT = "https://knuckles-team.github.io/ontology/"
Q32_ONE = 1 << 32
#: Only the learned reward orders the designations: the cosine and prior
#: weights are zero, so the ranking shows exactly what the outcomes taught.
REWARD_ONLY = {"cosine_q32": 0, "reward_q32": Q32_ONE, "prior_q32": 0}


def _classes_pack(connector: str, parent: str, child: str):
    turtle = (
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
        f"<{parent}> a owl:Class ; rdfs:subClassOf <{ONT}core#Agreement> .\n"
        f"<{child}> a owl:Class ; rdfs:subClassOf <{parent}> .\n"
    )
    return [ontology_entry(connector, "capabilities", turtle)]


async def _publish(
    client: Any,
    component_id: str,
    kind: str,
    capabilities: list[str],
    tags: list[str],
) -> None:
    """One Agent Library component with declared capabilities and policy tags."""
    draft = publication(component_id, "feature_schema", _EMPTY_BODY)
    component = draft["op"]["request"]["component"]
    for key in ds.body_attributes_for_publish(_EMPTY_BODY):
        component.pop(key, None)
    component.update(
        kind=kind,
        summary=f"{kind} {component_id}",
        declared_capabilities=capabilities,
        classification=tags,
    )
    await client._send("AgentComponent", draft)


def _empty_body() -> Any:
    from _decision_resident_fixture import schema_body

    return schema_body()


_EMPTY_BODY = _empty_body()


async def _designate(
    decision: dc.DecisionClient,
    schema: dict[str, Any],
    graph: str,
    space: dict[str, Any],
    required: list[str],
    tags: list[str],
) -> dict[str, Any]:
    question = {
        "tenant_id": TEST_TENANT,
        "query_vector": vector(space["space_digest"]),
        "required_capabilities": required,
        "required_policy_tags": tags,
        "k": 5,
        "weights": REWARD_ONLY,
        "ontology_prior": None,
    }
    request = dc.capability_designation_request(
        TEST_TENANT, f"designate-{uuid.uuid4().hex[:8]}", question, graph, schema
    )
    return (await decision.designate(request))["records"][0]


async def _await_designated(
    decision: dc.DecisionClient,
    schema: dict[str, Any],
    graph: str,
    space: dict[str, Any],
    required: list[str],
    tags: list[str],
    expected: set[str],
) -> dict[str, Any]:
    """Ask until the feed has indexed the published components."""
    deadline = time.monotonic() + 180
    while True:
        record = await _designate(decision, schema, graph, space, required, tags)
        answer = (record.get("answer") or {}).get("value") or {}
        found = {d["entity_id"] for d in answer.get("designations", [])}
        if found == expected:
            return record
        assert time.monotonic() < deadline, (found, expected, record)
        await asyncio.sleep(0.5)


def _order(record: dict[str, Any]) -> list[str]:
    return [d["entity_id"] for d in record["answer"]["value"]["designations"]]


def _rewards(record: dict[str, Any]) -> dict[str, int]:
    return {
        d["entity_id"]: d["reward_q32"]
        for d in record["answer"]["value"]["designations"]
    }


async def _outcomes(
    decision: dc.DecisionClient, entity: str, rewards: list[int], tag: str
) -> None:
    for index, reward in enumerate(rewards):
        applied = await decision.log(
            dc.capability_outcome_op(
                TEST_TENANT,
                entity,
                f"{tag}-{entity}-{index}",
                reward_q32=reward,
            )
        )
        assert applied["replayed"] is False, applied


# spec: MEDULLA-DECISION-ENGINE-R172.11
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R172.11")
@pytest.mark.no_engine
def test_designation_end_to_end(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    graph = f"designation-{suffix}"
    connector = f"cd-{suffix}"
    parent = f"https://example.org/{connector}/Lookup"
    child = f"https://example.org/{connector}/OrderLookup"
    other = f"https://example.org/{connector}/Dispatch"
    ids = {
        "agent": f"agent-{suffix}",
        "tool_a": f"tool-a-{suffix}",
        "tool_b": f"tool-b-{suffix}",
        "tool_untagged": f"tool-untagged-{suffix}",
        "skill": f"skill-{suffix}",
    }

    async def first_run(engine: Engine) -> tuple[dict[str, Any], dict[str, Any]]:
        client = await connect(engine=engine)
        try:
            space = await establish_commons_generation(client)
            schema = await publish_schema(client)
            await client.tenants.create(graph)
        finally:
            await client.close()
        client = await connect(graph, engine=engine)
        decision = dc.DecisionClient(client=client, graph=graph)
        try:
            imported = await import_entries(
                client, connector, _classes_pack(connector, parent, child)
            )
            assert imported["result"] == "imported", imported
            await attach_pack(client, connector)
            # The tools and the skill declare the subclass; the agent and a
            # tool without the policy tag must never be designated.
            await _publish(client, ids["tool_a"], "tool", [child], ["public"])
            await _publish(client, ids["tool_b"], "tool", [child], ["public"])
            await _publish(client, ids["tool_untagged"], "tool", [child], [])
            await _publish(client, ids["skill"], "skill", [other], ["public"])
            await _publish(client, ids["agent"], "agent", [child], ["public"])

            # Subsumption: requiring the superclass designates the tools that
            # declare the subclass; the policy tag filters the untagged one;
            # the skill declares an unrelated class; the agent is not a
            # designatable kind.
            record = await _await_designated(
                decision,
                schema,
                graph,
                space,
                [parent],
                ["public"],
                {ids["tool_a"], ids["tool_b"]},
            )
            designations = record["answer"]["value"]["designations"]
            assert all(d["eligibility"]["eligible"] for d in designations), designations
            assert all(d["eligibility"]["subsumption_paths"] for d in designations), (
                designations
            )
            assert ids["tool_untagged"] not in _order(record)

            # Learned outcomes re-rank: a leads after a success-heavy history,
            # then b overtakes it after the opposite one.
            await _outcomes(decision, ids["tool_a"], [Q32_ONE], "one")
            await _outcomes(decision, ids["tool_b"], [0], "one")
            record = await _designate(
                decision, schema, graph, space, [parent], ["public"]
            )
            assert _order(record) == [ids["tool_a"], ids["tool_b"]], record
            await _outcomes(decision, ids["tool_a"], [0, 0, 0], "two")
            await _outcomes(decision, ids["tool_b"], [Q32_ONE, Q32_ONE, Q32_ONE], "two")
            record = await _designate(
                decision, schema, graph, space, [parent], ["public"]
            )
            assert _order(record) == [ids["tool_b"], ids["tool_a"]], record
            rewards = _rewards(record)
            assert rewards[ids["tool_b"]] > rewards[ids["tool_a"]], rewards
            return space, rewards
        finally:
            await client.close()

    async def second_run(
        engine: Engine, space: dict[str, Any], rewards: dict[str, int]
    ) -> None:
        client = await connect(engine=engine)
        try:
            schema = await publish_schema(client)
        finally:
            await client.close()
        client = await connect(graph, engine=engine)
        decision = dc.DecisionClient(client=client, graph=graph)
        try:
            # The learned rewards are durable: the restarted engine ranks the
            # same designations with the same rewards.
            record = await _await_designated(
                decision,
                schema,
                graph,
                space,
                [parent],
                ["public"],
                {ids["tool_a"], ids["tool_b"]},
            )
            assert _order(record) == [ids["tool_b"], ids["tool_a"]], record
            assert _rewards(record) == rewards
        finally:
            await client.close()

    with restartable_engine(tmp_path, "first", bootstrap=True) as engine:
        space, rewards = asyncio.run(first_run(engine))
    with restartable_engine(tmp_path, "restart", bootstrap=False) as engine:
        asyncio.run(second_run(engine, space, rewards))
