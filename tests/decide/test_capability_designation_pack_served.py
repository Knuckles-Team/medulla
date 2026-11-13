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
import base64
import json
import struct
import sys
import time
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

import os

import pytest
from _decision_resident_fixture import publication, schema_body
from conftest import TEST_TENANT, request_context
from ontology_pack_support import attach_pack, import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind, PackHeadRef

COMMONS = "__commons__"
ONT = "https://knuckles-team.github.io/ontology/"
Q32_ONE = 1 << 32
DIM = 384


def _connect(graph: str | None = None):
    return MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )


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


def _vector(version: str) -> dict[str, Any]:
    raw = struct.pack(f"<{DIM}f", *([1.0] + [0.0] * (DIM - 1)))
    return {
        "embedding_version": version,
        "dim": DIM,
        "f32le_b64": base64.b64encode(raw).decode(),
    }


async def _commons_space(decision: dc.DecisionClient, model: dict[str, Any]):
    return await decision.log(
        {
            "op": "generation_space",
            "tenant_id": TEST_TENANT,
            "logical": COMMONS,
            "model": model,
        }
    )


async def _establish_commons_generation(client: MedullaClient) -> dict[str, Any]:
    """The commons generation, established through the served cycle when absent."""
    decision = dc.DecisionClient(client=client)
    model = {"id": "pack-designation-model", "revision": "1", "dimension": DIM}
    try:
        await client.tenants.create(COMMONS)
    except EngineResponseError:
        pass  # already exists
    read = await _commons_space(decision, model)
    if read["active"] is None:
        cell = f"cell-{uuid.uuid4().hex[:8]}"
        await client.capacity_leases.update_cell(
            {
                "schema_version": "1",
                "cell": {
                    "cell_id": cell,
                    "resource_class": "llm_embedding",
                    "capacity": 8,
                    "reserved_floor": 0,
                    "epoch": 1,
                    "policy_digest": "sha256:" + "d" * 64,
                    "updated_at_ms": int(time.time() * 1000),
                },
                "expected_epoch": None,
                "now_ms": int(time.time() * 1000),
            }
        )
        shadow = f"commons-gen-{uuid.uuid4().hex[:8]}"
        await client.tenants.create(shadow)
        cycle = dc.learn_op(
            TEST_TENANT,
            "run_generation_cycle",
            request={
                "model_ref": model,
                "evaluation": {
                    "logical": COMMONS,
                    "active_graph": COMMONS,
                    "active_space": "none",
                    "shadow_graph": shadow,
                    "shadow_space": read["model_space_digest"],
                    "items": [],
                    "top_k": 5,
                    "min_eval_items": 5,
                    "max_score_psi_q16": 64 << 16,
                },
                "capacity_cell": cell,
                "max_batches": None,
            },
        )
        job = await decision.log(cycle)
        assert job["recorded"] == "cycle", job
        assert job["cycle"]["state"] == "activated", job
        read = await _commons_space(decision, model)
        assert read["active"]["graph"] == shadow, read
        assert read["active"]["space_digest"] == read["model_space_digest"], read
    assert read["active"]["dimensions"] == DIM, read
    return read["active"]


async def _publish_schema(client: MedullaClient) -> dict[str, Any]:
    component_id = "pack-designation-schema-" + uuid.uuid4().hex
    body = schema_body()
    result = await client._send(
        "AgentComponent", publication(component_id, "feature_schema", body)
    )
    return {
        "component_id": component_id,
        "kind": "feature_schema",
        "definition_digest": result["component"]["definition_digest"],
    }


async def _ask(
    decision: dc.DecisionClient,
    schema: dict[str, Any],
    graph: str,
    space: dict[str, Any],
    target: str,
) -> dict[str, Any]:
    question = {
        "tenant_id": TEST_TENANT,
        "query_vector": _vector(space["space_digest"]),
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
        client = await _connect()
        try:
            space = await _establish_commons_generation(client)
            schema = await _publish_schema(client)
        finally:
            await client.close()
        suffix = uuid.uuid4().hex[:8]
        graph = f"pack-designation-{suffix}"
        admin = await _connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await _connect(graph)
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
