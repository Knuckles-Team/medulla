"""Served-test kit for the commons embedding generation: the connection, the
generation the first served cycle establishes, the query vector in its space and
a published feature schema (MEDULLA-DECISION-ENGINE-R172.12.6, R172.11)."""

from __future__ import annotations

import base64
import os
import struct
import time
import uuid
from typing import Any

from _decision_resident_fixture import publication, schema_body
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient

COMMONS = "__commons__"
DIM = 384


def connect(graph: str | None = None):
    return MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )


def vector(version: str) -> dict[str, Any]:
    raw = struct.pack(f"<{DIM}f", *([1.0] + [0.0] * (DIM - 1)))
    return {
        "embedding_version": version,
        "dim": DIM,
        "f32le_b64": base64.b64encode(raw).decode(),
    }


async def commons_space(decision: dc.DecisionClient, model: dict[str, Any]):
    return await decision.log(
        {
            "op": "generation_space",
            "tenant_id": TEST_TENANT,
            "logical": COMMONS,
            "model": model,
        }
    )


async def establish_commons_generation(client: MedullaClient) -> dict[str, Any]:
    """The commons generation, established through the served cycle when absent."""
    decision = dc.DecisionClient(client=client)
    model = {"id": "pack-designation-model", "revision": "1", "dimension": DIM}
    try:
        await client.tenants.create(COMMONS)
    except EngineResponseError:
        pass  # already exists
    read = await commons_space(decision, model)
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
        read = await commons_space(decision, model)
        assert read["active"]["graph"] == shadow, read
        assert read["active"]["space_digest"] == read["model_space_digest"], read
    assert read["active"]["dimensions"] == DIM, read
    return read["active"]


async def publish_schema(client: MedullaClient) -> dict[str, Any]:
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
