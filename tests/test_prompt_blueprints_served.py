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
    AgentComponentOpRecordSourceEffect,
)
from medulla.generated.connector_pack import (  # noqa: E402
    PackAnnotations,
    PackEntryKind,
)
from medulla.generated.storage import (  # noqa: E402
    send_agent_component_export_blueprint,
    send_agent_component_record_source_effect,
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


def _effect(
    component_id: str, revision: int, receipt_digest: str, **override: Any
) -> AgentComponentOpRecordSourceEffect:
    fields: dict[str, Any] = {
        "op": "record_source_effect",
        "tenant_id": TEST_TENANT,
        "component_id": component_id,
        "revision": revision,
        "repository": "prompt-repository",
        "path": "prompts/lossless.json",
        "effect_revision": "c0ffee1",
        "request_digest": hashlib.sha256(b"request").hexdigest(),
        "receipt_digest": receipt_digest,
    }
    fields.update(override)
    return AgentComponentOpRecordSourceEffect(**fields)


async def _record(client: MedullaClient, effect: AgentComponentOpRecordSourceEffect):
    return await send_agent_component_record_source_effect(
        client, effect, idempotency_key=f"source-effect-{uuid.uuid4().hex}"
    )


# spec: MEDULLA-TYPED-PACKS-R094.8
@pytest.mark.asyncio
async def test_source_effect_receipt_is_recorded_once() -> None:
    connector = f"blueprint-{uuid.uuid4().hex[:10]}"
    name = "effect-prompt"
    client = await connect()
    try:
        await _import_prompt(client, connector, name)
        component_id = _component_id(connector, name)
        digest = hashlib.sha256(b"receipt").hexdigest()

        first = await _record(client, _effect(component_id, 1, digest))
        assert first.recorded is True
        assert first.receipt_digest == digest
        # The same receipt again writes nothing.
        replay = await _record(client, _effect(component_id, 1, digest))
        assert replay.recorded is False

        # The same digest with other bytes refuses by name.
        with pytest.raises(EngineResponseError, match="RECEIPT_DIGEST_REUSED"):
            await _record(
                client, _effect(component_id, 1, digest, path="prompts/other.json")
            )
        with pytest.raises(EngineResponseError, match="RECEIPT_REVISION_UNKNOWN"):
            await _record(client, _effect(component_id, 99, digest))
        with pytest.raises(EngineResponseError, match="RECEIPT_DIGEST_INVALID"):
            await _record(client, _effect(component_id, 1, "NOT-A-DIGEST"))
        # A cross-tenant caller refuses.
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _record(
                client, _effect(component_id, 1, digest, tenant_id="tenant-other")
            )
    finally:
        await client.close()


class _LoopbackRepositoryConnector:
    """The repository connector arbora's ``fleet.write_back.commit`` reaches.

    It stands in for the provider: it accepts only an effect whose approval is bound
    to the exact payload digest, holds the "repository" in memory, and answers with a
    receipt binding the approved request, the actual revision it produced and the
    digest of the content it wrote. Medulla writes no file; it only keeps the receipt.
    """

    def __init__(self) -> None:
        self.files: dict[str, bytes] = {}

    def commit(
        self, path: str, payload: bytes, approved_digest: str | None
    ) -> dict[str, str]:
        payload_digest = hashlib.sha256(payload).hexdigest()
        if approved_digest != payload_digest:
            raise PermissionError("write-back refused: approval does not bind payload")
        self.files[path] = payload
        request_digest = hashlib.sha256(f"{path}\0{payload_digest}".encode()).hexdigest()
        effect_revision = hashlib.sha256(payload + path.encode()).hexdigest()[:12]
        receipt_digest = hashlib.sha256(
            f"{request_digest}\0{effect_revision}\0{payload_digest}".encode()
        ).hexdigest()
        return {
            "request_digest": request_digest,
            "effect_revision": effect_revision,
            "receipt_digest": receipt_digest,
        }


# spec: MEDULLA-TYPED-PACKS-R094.7
@pytest.mark.asyncio
async def test_approved_source_effect_dependency() -> None:
    connector = f"blueprint-{uuid.uuid4().hex[:10]}"
    name = "approved-prompt"
    client = await connect()
    try:
        await _import_prompt(client, connector, name)
        component_id = _component_id(connector, name)
        exported = await _export(client, component_id, 1)
        provider = _LoopbackRepositoryConnector()
        path = "prompts/approved-prompt.json"

        # Without approval bound to the exact payload nothing is written or recorded.
        with pytest.raises(PermissionError):
            provider.commit(path, exported.body, approved_digest=None)
        assert provider.files == {}

        receipt = provider.commit(
            path,
            exported.body,
            approved_digest=exported.content_digest.removeprefix("sha256:"),
        )
        assert provider.files[path] == exported.body

        recorded = await _record(
            client,
            _effect(
                component_id,
                exported.entry.entry_revision,
                receipt["receipt_digest"],
                repository="prompt-repository",
                path=path,
                effect_revision=receipt["effect_revision"],
                request_digest=receipt["request_digest"],
            ),
        )
        assert recorded.recorded is True
        assert recorded.revision == 1
    finally:
        await client.close()
