"""Agent component lifecycle over the wire (MEDULLA-DECISION-ENGINE-R168.4/.5/.6).

A loopback engine with authentication serves ``AgentComponent``. The tests
construct, transition, store, read back and serialize one component, and submit
the malformed cases: a second retire, a publish over the tombstone. A refusal
writes nothing, so the revision history is unchanged after each one.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context
from test_canonical_body_served import _library_context, _skill_draft

from medulla.client import MedullaClient
from medulla.generated.agent_component import AgentComponentEntry
from medulla.generated.storage import send_agent_component


async def _client() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _new_id() -> str:
    return f"skill:lifecycle-{uuid.uuid4().hex[:12]}"


async def _publish(client: MedullaClient, component_id: str, expected: int) -> None:
    await send_agent_component(
        client,
        {
            "op": {
                "op": "publish",
                "request": {
                    "context": _library_context(expected),
                    "component": _skill_draft(component_id),
                    "evaluation_receipt_digest": None,
                },
            }
        },
        idempotency_key=f"lifecycle-publish-{uuid.uuid4().hex}",
    )


async def _retire(client: MedullaClient, component_id: str, expected: int) -> None:
    await send_agent_component(
        client,
        {
            "op": {
                "op": "retire",
                "request": {
                    "context": _library_context(expected),
                    "component_id": component_id,
                },
            }
        },
        idempotency_key=f"lifecycle-retire-{uuid.uuid4().hex}",
    )


async def _history(client: MedullaClient, component_id: str) -> list[dict[str, Any]]:
    result = await send_agent_component(
        client,
        {
            "op": {
                "op": "history",
                "tenant_id": TEST_TENANT,
                "component_id": component_id,
            }
        },
    )
    assert isinstance(result.payload, list)
    return result.payload


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R168.4")
@pytest.mark.asyncio
async def test_malformed_lifecycle_refuses() -> None:
    client = await _client()
    component_id = _new_id()
    try:
        await _publish(client, component_id, 0)
        await _retire(client, component_id, 1)
        before = await _history(client, component_id)
        assert [entry["lifecycle"] for entry in before] == ["published", "retired"]

        # Retired is terminal: neither a second retire nor a publish moves it.
        with pytest.raises(RuntimeError, match="LIFECYCLE_RETIRED_TERMINAL"):
            await _retire(client, component_id, 2)
        with pytest.raises(RuntimeError, match="LIFECYCLE_RETIRED_TERMINAL"):
            await _publish(client, component_id, 2)

        # A refusal writes nothing: no revision was appended.
        assert await _history(client, component_id) == before
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R168.5")
@pytest.mark.asyncio
async def test_transition_entry_names_states_and_digest() -> None:
    client = await _client()
    component_id = _new_id()
    try:
        await _publish(client, component_id, 0)
        await _publish(client, component_id, 1)
        await _retire(client, component_id, 2)
        history = await _history(client, component_id)
        # Each accepted transition is one more revision of the same stream.
        assert [entry["entry_revision"] for entry in history] == [1, 2, 3]
        pairs = list(zip(history, history[1:], strict=False))
        assert [(a["lifecycle"], b["lifecycle"]) for a, b in pairs] == [
            ("published", "published"),
            ("published", "retired"),
        ]
        for _prior, entry in pairs:
            assert entry["definition_digest"].startswith("sha256:")
        # The tombstone preserves its prior definition's digest.
        assert history[2]["definition_digest"] == history[1]["definition_digest"]
    finally:
        await client.close()


@pytest.mark.spec(
    "MEDULLA-DECISION-ENGINE-R168",
    "MEDULLA-DECISION-ENGINE-R168.1",
    "MEDULLA-DECISION-ENGINE-R168.2",
    "MEDULLA-DECISION-ENGINE-R168.6",
)
@pytest.mark.asyncio
async def test_agent_lifecycle_roundtrip() -> None:
    client = await _client()
    component_id = _new_id()
    try:
        # Construction and storage.
        await _publish(client, component_id, 0)
        stored = await _history(client, component_id)
        assert len(stored) == 1
        # Serialization: the stored record validates as the contract entry and
        # survives a dump and reload unchanged.
        entry = AgentComponentEntry.model_validate(stored[0])
        assert entry.lifecycle.value == "published"
        again = AgentComponentEntry.model_validate(entry.model_dump(mode="json"))
        assert again == entry
        # Transition, then read back.
        await _retire(client, component_id, 1)
        current = (await _history(client, component_id))[-1]
        assert current["lifecycle"] == "retired"
        assert current["component_id"] == component_id
        # Malformed case.
        with pytest.raises(RuntimeError, match="LIFECYCLE_RETIRED_TERMINAL"):
            await _publish(client, component_id, 2)
    finally:
        await client.close()
