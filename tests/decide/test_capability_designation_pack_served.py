"""MEDULLA-DECISION-ENGINE-R172.12.6: pack designation over the wire.

The commons embedding generation is established through the served generation
cycle (the first generation bootstraps), its space is read through
``DecisionLog.generation_space``, a connector pack is imported through
``ConnectorPack.Import`` and the served ``Decide`` designation question names
the tool that applies to a subclass of the target class, with the subclass path
in its premises. After the pack is re-imported without the tool the same
question abstains.
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


import pytest
from _commons_generation import (
    connect,
    establish_commons_generation,
    publish_schema,
    vector,
)
from conftest import TEST_TENANT
from ontology_pack_support import attach_pack, import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla import decision_client as dc
from medulla.client import EngineResponseError
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind, PackHeadRef

ONT = "https://knuckles-team.github.io/ontology/"
Q32_ONE = 1 << 32


def _tool(connector: str, name: str, text: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps({"name": name, "description": text}).encode(),
        input_schema=b'{"properties":{"q":{"type":"string"}},"type":"object"}',
        annotations=PackAnnotations(),
    )


def _pack(connector: str, classes: dict[str, str], with_tool_a: bool):
    order, sub, ship = classes["order"], classes["sub"], classes["ship"]
    turtle = (
        f"@prefix cp: <{ONT}connector-pack#> .\n"
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
        f"<{order}> a owl:Class ; rdfs:subClassOf <{ONT}core#Agreement> .\n"
        f"<{sub}> a owl:Class ; rdfs:subClassOf <{order}> .\n"
        f"<{ship}> a owl:Class ; rdfs:subClassOf <{ONT}core#Agreement> .\n"
        f"<tool://{connector}/a> cp:operatesOn <{sub}> .\n"
        f"<tool://{connector}/b> cp:executes <{ship}> .\n"
    )
    entries = [_tool(connector, "b", "dispatch shipment")]
    if with_tool_a:
        entries.append(_tool(connector, "a", "look up an order"))
    entries.append(ontology_entry(connector, "surface", turtle))
    return entries


async def _ask(
    decision: dc.DecisionClient,
    schema: dict[str, Any],
    graph: str,
    space: dict[str, Any],
    target: str,
) -> dict[str, Any]:
    question = {
        "tenant_id": TEST_TENANT,
        "query_vector": vector(space["space_digest"]),
        "required_capabilities": [],
        "required_policy_tags": [],
        "k": 3,
        "weights": {"cosine_q32": 0, "reward_q32": 0, "prior_q32": Q32_ONE},
        "ontology_prior": None,
        "target_classes": [target],
    }
    request = dc.capability_designation_request(
        TEST_TENANT, f"designate-{uuid.uuid4().hex[:8]}", question, graph, schema
    )
    return (await decision.designate(request))["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R172.12.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R172.12.6")
def test_designation_reads_imported_pack() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            space = await establish_commons_generation(client)
            schema = await publish_schema(client)
        finally:
            await client.close()
        suffix = uuid.uuid4().hex[:8]
        graph = f"pack-designation-{suffix}"
        admin = await connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await connect(graph)
        decision = dc.DecisionClient(client=client, graph=graph)
        try:
            connector = f"pd-{suffix}"
            classes = {
                "order": f"https://example.org/{connector}/Order",
                "sub": f"https://example.org/{connector}/PurchaseOrder",
                "ship": f"https://example.org/{connector}/Ship",
            }
            imported = await import_entries(
                client, connector, _pack(connector, classes, with_tool_a=True)
            )
            assert imported["result"] == "imported", imported
            await attach_pack(client, connector)
            tool_a = f"mcp:{connector}/tool/a"

            deadline = time.monotonic() + 180
            while True:
                record = await _ask(decision, schema, graph, space, classes["order"])
                answer = (record.get("answer") or {}).get("value") or {}
                designated = [d["entity_id"] for d in answer.get("designations", [])]
                if designated == [tool_a]:
                    break
                assert time.monotonic() < deadline, (designated, record)
                await asyncio.sleep(0.5)
            assert answer["designations"][0]["applies_to"] == [classes["sub"]]
            path = f"{classes['sub']}>{classes['order']}"
            facts = [p["fact"] for p in record["premises"]]
            assert any(
                fact.startswith(f"target_designation:component={tool_a};path={path};")
                for fact in facts
            ), facts

            head = PackHeadRef(
                binding_revision=imported["receipt"]["binding_revision"],
                pack_digest=imported["receipt"]["pack_digest"],
            )
            withdrawn = await import_entries(
                client,
                connector,
                _pack(connector, classes, with_tool_a=False),
                expected_head=head,
            )
            assert withdrawn["result"] == "imported", withdrawn
            deadline = time.monotonic() + 180
            while True:
                try:
                    record = await _ask(
                        decision, schema, graph, space, classes["order"]
                    )
                except EngineResponseError as error:
                    # The index may still hold the older pack revision.
                    assert "DESIGNATION_STALE_PACK_REVISION" in str(error), error
                else:
                    if record["resolution_kind"] == "abstention":
                        break
                assert time.monotonic() < deadline, record
                await asyncio.sleep(0.5)
            assert "DESIGNATION_NO_DESIGNATED_COMPONENT" in json.dumps(
                record["outcome"]
            ), record["outcome"]
        finally:
            await client.close()

    asyncio.run(exercise())
