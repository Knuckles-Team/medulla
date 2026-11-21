"""Served measured fact write (MEDULLA-ORCH-VOCAB-R011.10).

An evaluation result reaches a ``ModelProfile`` once, through its publish: the
first record adds revision 2, a repeat is a replay that adds nothing, and an empty
evaluation id is refused by the engine.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context
from test_canonical_body_served import _library_context, _skill_draft
from test_harness_config_served import (
    PROFILE as _CONFIG_PROFILE,
    SWARM,
    TASK,
    _evidence,
    _publish_model_profile,
    _request,
    _seed_graph,
)

from medulla.client import MedullaClient
from medulla.decision_client import MeasuredFactRecorded, record_measured_fact
from medulla.generated.storage import send_agent_component


async def _client() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _profile_draft(component_id: str) -> dict[str, Any]:
    draft = _skill_draft(component_id)
    draft["kind"] = "model_profile"
    draft["facts"] = {
        "facts": "model_profile",
        "provider": "fixture",
        "model_identity": "fixture/model",
        "context_window_tokens": 8192,
        "max_output_tokens": 1024,
        "supports_tools": True,
        "supports_structured_output": True,
        "supports_vision": False,
    }
    return draft


async def _head(client: MedullaClient, component_id: str) -> dict[str, Any]:
    result = await send_agent_component(
        client,
        {
            "op": {
                "op": "current",
                "tenant_id": TEST_TENANT,
                "component_id": component_id,
            }
        },
    )
    assert isinstance(result.payload, dict)
    return result.payload


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R011.10")
@pytest.mark.asyncio
async def test_evaluation_result_publishes_one_profile_revision():
    client = await _client()
    ref = f"model-profile:measured-{uuid.uuid4().hex[:12]}"
    digest = "sha256:" + "e" * 64
    try:
        await send_agent_component(
            client,
            {
                "op": {
                    "op": "publish",
                    "request": {
                        "context": _library_context(0),
                        "component": _profile_draft(ref),
                        "evaluation_receipt_digest": None,
                    },
                }
            },
            idempotency_key=f"measured-fact-seed-{ref}",
        )

        recorded = await record_measured_fact(
            client, ref, "shell_proficiency", "high", "eval:shell", digest
        )
        assert recorded == MeasuredFactRecorded(ref, 2, False)
        head = await _head(client, ref)
        assert head["entry_revision"] == 2
        assert head["facts"]["shell_proficiency"] == {
            "value": "high",
            "evaluation_run_ref": {"evaluation_id": "eval:shell", "digest": digest},
        }

        # A retried result adds no revision.
        repeat = await record_measured_fact(
            client, ref, "shell_proficiency", "high", "eval:shell", digest
        )
        assert repeat == MeasuredFactRecorded(ref, 2, True)
        assert (await _head(client, ref))["entry_revision"] == 2

        # A fact with no valid run reference is the engine's refusal.
        with pytest.raises(Exception):
            await record_measured_fact(
                client, ref, "stopping_behaviour", "balanced", "", digest
            )
        assert (await _head(client, ref))["entry_revision"] == 2

        # A profile that does not exist is refused before any write.
        with pytest.raises(ValueError, match="MODEL_PROFILE_UNKNOWN"):
            await record_measured_fact(
                client, f"{ref}:absent", "shell_proficiency", "low", "eval:x", digest
            )
    finally:
        await client.close()


def _decide_config(client: Any, schema: dict[str, Any], profile: str, graph: str) -> dict:
    return client._send("Decide", {"request": _request(schema, profile, graph)})[
        "records"
    ][0]


def _shell_premise(record: dict[str, Any]) -> dict[str, Any] | None:
    for premise in record["premises"]:
        if premise["fact"].startswith("harness_config.shell_proficiency="):
            return premise
    return None


async def _record_fact(profile: str, value: str, evaluation_id: str) -> Any:
    client = await _client()
    try:
        return await record_measured_fact(
            client,
            profile,
            "shell_proficiency",
            value,
            evaluation_id,
            "sha256:" + evaluation_id.encode().hex().ljust(64, "0")[:64],
        )
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R011.11")
def test_harness_config_cites_fact_and_abstains_without(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R011.11
    client = clean_graph
    graph = _graph_name(client)
    _seed_graph(
        client,
        "r011-11:facts",
        [
            (
                TASK,
                {f"{SWARM}shellCentric": False, f"{SWARM}taskTokenUpperBound": 10000},
            ),
            (_CONFIG_PROFILE, {}),
        ],
    )
    profile = f"{_CONFIG_PROFILE}-{uuid.uuid4().hex[:8]}"
    _publish_model_profile(client, profile, None, "balanced")
    schema = publish(client, f"r011-11-schema-{uuid.uuid4().hex[:8]}", "feature_schema", schema_body())

    # Without the fact the answer abstains with its conservative default.
    record = _decide_config(client, schema, profile, graph)
    action_space = _evidence(record)["answer"]["action_space"]
    assert action_space["decision"] == "abstained"
    assert action_space["conservative_default"] == "tool_set"
    assert _shell_premise(record) is None

    # The recorded fact is cited as an Observation premise of its evaluation.
    recorded = asyncio.run(_record_fact(profile, "high", "eval:shell-1"))
    assert recorded.entry_revision == 2
    record = _decide_config(client, schema, profile, graph)
    premise = _shell_premise(record)
    assert premise is not None
    assert premise["provenance"] == "observation"
    assert premise["evaluation_id"] == "eval:shell-1"

    # A newer profile revision replaces the cited fact.
    asyncio.run(_record_fact(profile, "low", "eval:shell-2"))
    record = _decide_config(client, schema, profile, graph)
    premise = _shell_premise(record)
    assert premise is not None
    assert premise["evaluation_id"] == "eval:shell-2"
