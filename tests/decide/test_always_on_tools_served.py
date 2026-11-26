"""MEDULLA-DECISION-ENGINE-R715.2: the always-on tools over the wire.

L4: every call goes through the running engine. One connector pack is imported through
``ConnectorPack.Import``. Once the capability feed has indexed it, a served ``Decide``
``load_set`` question with ``max_tools`` 3 returns the ranked tools inside the bound and
then the two always-on capabilities, ``knowledge.flag`` and ``skills.friction``,
outside it. The same question with a tenant policy that removes one of them
(``removed_capabilities``) refuses ``ALWAYS_ON_TOOL_REMOVED``. The tool-subset leg of
the row (``max_tools`` 1) is proven at L3 by
``crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs`` and
``tool_subset_tests.rs``.
"""

from __future__ import annotations

import asyncio
import hashlib
import struct
import sys
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
sys.path.insert(0, str(Path(__file__).resolve().parent / "fixtures"))

import pytest
from _commons_generation import (
    DIM,
    connect,
    establish_commons_generation,
    publish_schema,
)
from conftest import TEST_TENANT
from ontology_pack_support import import_entries
from pack_generator import make_packs

from medulla import decision_client as dc

MAX_TOOLS = 3
ALWAYS_ON = ["knowledge.flag", "skills.friction"]


def _question(
    space: dict[str, Any], graph: str, removed: list[str] | None = None
) -> dict[str, Any]:
    question: dict[str, Any] = {
        "job_text": "look up an order and close the ticket",
        "job_embedding": [1.0] + [0.0] * (DIM - 1),
        "required_capabilities": [],
        "max_tools": MAX_TOOLS,
        "max_agents": 1,
        "graph": graph,
        "embedding_version": space["space_digest"],
    }
    if removed:
        question["removed_capabilities"] = removed
    return question


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


async def _ask(
    decision: dc.DecisionClient,
    schema: dict[str, Any],
    graph: str,
    space: dict[str, Any],
    removed: list[str] | None = None,
) -> dict[str, Any]:
    request = dc.load_set_request(
        TEST_TENANT,
        f"always-on-{uuid.uuid4().hex[:8]}",
        _question(space, graph, removed),
        schema,
    )
    return (await decision.decide(request))["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R715.2
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R715.2")
def test_load_set_keeps_the_two_tools_and_a_policy_cannot_remove_them() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            space = await establish_commons_generation(client)
            schema = await publish_schema(client)
        finally:
            await client.close()
        graph = f"always-on-{uuid.uuid4().hex[:8]}"
        admin = await connect()
        try:
            await admin.tenants.create(graph)
        finally:
            await admin.close()
        client = await connect(graph)
        decision = dc.DecisionClient(client=client)
        try:
            for connector, entries in make_packs(count=1, tools=30):
                imported = await import_entries(client, connector, entries)
                assert imported["result"] == "imported", imported
                # The import settles asynchronously: the engine answers once the pack
                # projection has applied this import's binding revision.
                awaited = await client.await_watermark(
                    f"projection:{_pack_graph(TEST_TENANT, connector)}",
                    imported["receipt"]["binding_revision"],
                    deadline_ms=240_000,
                )
                assert awaited["reached"], awaited

            record = await _ask(decision, schema, graph, space)
            answer = (record.get("answer") or {}).get("value") or {}
            assert answer.get("tools"), record

            # The ranked tools stay inside max_tools; the two capabilities follow them.
            assert answer["tools"][-2:] == ALWAYS_ON, answer["tools"]
            ranked = answer["tools"][:-2]
            assert 0 < len(ranked) <= MAX_TOOLS, ranked
            assert not set(ranked) & set(ALWAYS_ON), ranked

            # A tenant policy that removes one of them refuses, by code.
            for removed in ALWAYS_ON:
                with pytest.raises(Exception, match="ALWAYS_ON_TOOL_REMOVED"):
                    await _ask(decision, schema, graph, space, [removed])
        finally:
            await client.close()

    asyncio.run(exercise())
