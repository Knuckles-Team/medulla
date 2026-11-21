"""Served prompt-blueprint acceptance (MEDULLA-TYPED-PACKS-R094.x).

A packaged prompt is exported as a native blueprint: the complete admitted
record at one pinned revision plus its body bytes, which travel verified and
never parsed. Each test imports a prompt through the served connector-pack
import and reads the blueprint back over the wire.
"""

from __future__ import annotations

import hashlib
import json
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent / "typed_packs"))

from ontology_pack_support import connect, import_entries  # noqa: E402

from conftest import TEST_TENANT  # noqa: E402  # isort: skip
from medulla import ConnectorPackEntryContent  # noqa: E402
from medulla.client import EngineResponseError, MedullaClient  # noqa: E402
from medulla.generated.agent_component import (  # noqa: E402
    AgentComponentBlueprintExportRequest,
)
from medulla.generated.connector_pack import (  # noqa: E402
    PackAnnotations,
    PackEntryKind,
)
from medulla.generated.storage import (  # noqa: E402
    send_agent_component_export_blueprint,
)

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "blueprint" / "lossless.json"


def _prompt_bytes() -> bytes:
    """The fixture bytes exactly as stored; never reparsed before import."""
    return FIXTURE.read_bytes()


def _prompt_entry(connector: str, name: str, body: bytes) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.PROMPT,
        uri=f"prompt://{name}",
        name=name,
        media_type="application/json",
        body=body,
        annotations=PackAnnotations(),
    )


def _component_id(connector: str, name: str) -> str:
    return f"mcp:{connector}/prompt/{name}"


async def _import_prompt(client: MedullaClient, connector: str, name: str) -> bytes:
    body = _prompt_bytes()
    result = await import_entries(
        client, connector, [_prompt_entry(connector, name, body)]
    )
    assert result["result"] == "imported", result
    return body


async def _export(
    client: MedullaClient, component_id: str, revision: int, tenant: str = TEST_TENANT
) -> Any:
    return await send_agent_component_export_blueprint(
        client,
        AgentComponentBlueprintExportRequest(
            tenant_id=tenant, component_id=component_id, entry_revision=revision
        ),
    )


# spec: MEDULLA-TYPED-PACKS-R094.2
@pytest.mark.asyncio
async def test_lossless_blueprint_export() -> None:
    connector = f"blueprint-{uuid.uuid4().hex[:10]}"
    name = "lossless-prompt"
    client = await connect()
    try:
        body = await _import_prompt(client, connector, name)
        component_id = _component_id(connector, name)
        exported = await _export(client, component_id, 1)
        # The body is the exact imported bytes, with no conversion.
        assert exported.body == body
        assert exported.content_digest == "sha256:" + hashlib.sha256(body).hexdigest()
        assert exported.entry.component_id == component_id
        assert exported.entry.entry_revision == 1
        assert exported.entry.tenant_id == TEST_TENANT
        assert exported.entry.content_digest == exported.content_digest
        assert exported.schema_version == 1
        # Number precision and nulls survive because the bytes are never parsed.
        assert b"9007199254740993" in exported.body
        assert json.loads(exported.body)["extensions"]["nothing"] is None

        with pytest.raises(EngineResponseError, match="COMPONENT_REVISION_UNKNOWN"):
            await _export(client, component_id, 99)
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _export(client, component_id, 1, tenant="tenant-other")
    finally:
        await client.close()
