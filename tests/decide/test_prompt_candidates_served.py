"""MEDULLA-DECISION-ENGINE-R814.8: a packaged prompt must match the canonical
blueprint, refused through the served ``ConnectorPack.import`` with a code that
names the field path and never the field's value."""

from __future__ import annotations

import os
from typing import Any

import pytest
from conftest import request_context
from test_connector_pack_planted import IMPORTER, _bind, _import

from medulla.client import MedullaClient
from medulla.testing.synthetic.packs.content import canonical_json
from medulla.testing.synthetic.packs.malformed import base_spec, edited

LEGACY_SECRET = "legacy-body-text-that-must-not-be-echoed"


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
