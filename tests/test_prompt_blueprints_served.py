"""Served prompt-blueprint acceptance (MEDULLA-TYPED-PACKS-R094.x).

A packaged prompt is exported as a native blueprint: the complete admitted
record at one pinned revision plus its body bytes, which travel verified and
never parsed. Each test imports a prompt through the served connector-pack
import and reads the blueprint back over the wire.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import struct
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent / "typed_packs"))

from ontology_pack_support import connect, import_entries

from conftest import TEST_TENANT  # isort: skip
from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.agent_component import (
    AgentComponentBlueprintExportRequest,
    AgentComponentContentRequest,
    AgentComponentOpCurrent,
    AgentComponentOpRecordSourceEffect,
)
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
)
from medulla.generated.storage import (
    send_agent_component,
    send_agent_component_content,
    send_agent_component_current,
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.2")
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.8")
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
        request_digest = hashlib.sha256(
            f"{path}\0{payload_digest}".encode()
        ).hexdigest()
        effect_revision = hashlib.sha256(payload + path.encode()).hexdigest()[:12]
        receipt_digest = hashlib.sha256(
            f"{request_digest}\0{effect_revision}\0{payload_digest}".encode()
        ).hexdigest()
        return {
            "request_digest": request_digest,
            "effect_revision": effect_revision,
            "receipt_digest": receipt_digest,
        }


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.7")
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


def _publish_context(expected_revision: int) -> dict[str, Any]:
    principal = "principal:sha256:" + "a" * 64
    return {
        "request_id": 0,
        "principal": principal,
        "caller_principal": principal,
        "attempt_nonce": uuid.uuid4().hex * 2,
        "tenant_id": TEST_TENANT,
        "actor_scope": "placeholder",
        "purpose_id": "placeholder",
        "policy_revision": "placeholder",
        "policy_digest": "sha256:" + "0" * 64,
        "policy_decision_id": "placeholder",
        "idempotency_key": uuid.uuid4().hex,
        "expected_revision": expected_revision,
        "trace_id": None,
        "created_at_ms": 0,
    }


def _prompt_draft(component_id: str, pin: str) -> dict[str, Any]:
    return {
        "component_id": component_id,
        "kind": "system_prompt",
        "version": "1.0.0",
        "content_digest": "sha256:" + pin,
        "content_ref": None,
        "facts": {
            "facts": "system_prompt",
            "prompt_mode": "static",
            "token_estimate": 16,
            "variables": [],
            "body_sha256": pin,
            "schema_revision": 1,
        },
        "provenance": {"origin": "native"},
        "summary": "natively published prompt",
        "attributes": {},
        "tenant_id": TEST_TENANT,
        "actor_scope": "placeholder",
        "purpose_id": "placeholder",
        "policy_digest": "sha256:" + "0" * 64,
        "source_revision": "rev-1",
        "source_revision_digest": "sha256:" + "8" * 64,
    }


async def _publish_prompt(
    client: MedullaClient,
    component_id: str,
    blueprint: str,
    pin: str,
    expected_revision: int,
    *,
    context: dict[str, Any] | None = None,
    idempotency_key: str | None = None,
) -> Any:
    request = {
        "context": context or _publish_context(expected_revision),
        "component": _prompt_draft(component_id, pin),
        "evaluation_receipt_digest": None,
        "prompt_blueprint": blueprint,
    }
    return await send_agent_component(
        client,
        {"op": {"op": "publish", "request": request}},
        idempotency_key=idempotency_key or f"prompt-publish-{uuid.uuid4().hex}",
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.7")
@pytest.mark.asyncio
async def test_prompt_owner_commit() -> None:
    client = await connect()
    component_id = f"prompt:native-{uuid.uuid4().hex[:12]}"
    blueprint = json.dumps(
        {"task": "summarise the ticket", "schema_revision": 1},
        sort_keys=True,
        separators=(",", ":"),
    )
    pin = hashlib.sha256(blueprint.encode()).hexdigest()
    try:
        await _publish_prompt(client, component_id, blueprint, pin, 0)
        current = await send_agent_component_current(
            client,
            AgentComponentOpCurrent(
                op="current", tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        assert current is not None and current.entry_revision == 1
        content = await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        assert content.body == blueprint.encode()
        assert content.content_digest == "sha256:" + pin

        # A second publish at the old expected revision is stale; the head stays at 1.
        with pytest.raises(EngineResponseError, match="STALE_AGENT_LIBRARY_REVISION"):
            await _publish_prompt(client, component_id, blueprint, pin, 0)
        current = await send_agent_component_current(
            client,
            AgentComponentOpCurrent(
                op="current", tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        assert current is not None and current.entry_revision == 1

        # A body whose hash differs from the pin is refused.
        other_id = f"prompt:native-{uuid.uuid4().hex[:12]}"
        with pytest.raises(EngineResponseError, match="BODY_DIGEST_MISMATCH"):
            await _publish_prompt(client, other_id, blueprint, "f" * 64, 0)

        # A blueprint with no task is refused by name.
        no_task = json.dumps({"schema_revision": 1}, separators=(",", ":"))
        no_task_pin = hashlib.sha256(no_task.encode()).hexdigest()
        with pytest.raises(EngineResponseError, match="PROMPT_SCHEMA_INVALID"):
            await _publish_prompt(client, other_id, no_task, no_task_pin, 0)
    finally:
        await client.close()


NATIVE_GRAPH_DOMAIN = b"eg/native-component-projection-graph/v1"
CORE = "https://knuckles-team.github.io/ontology/core#"


def _native_graph(tenant: str) -> str:
    """The physical native-component projection graph: ``Digest256::framed`` of the tenant."""
    hasher = hashlib.sha256()
    hasher.update(b"eg/framed-sha256/v1\0")
    hasher.update(struct.pack(">I", len(NATIVE_GRAPH_DOMAIN)))
    hasher.update(NATIVE_GRAPH_DOMAIN)
    field = tenant.encode()
    hasher.update(struct.pack(">I", 1))
    hasher.update(struct.pack(">Q", len(field)))
    hasher.update(field)
    return f"component__{hasher.hexdigest()}"


async def _projected_revisions(
    graph: str, component_id: str, want: int, timeout_s: float = 120.0
) -> dict[int, str]:
    """The ``PromptRevision`` nodes of one component by entry revision, once ``want`` exist."""
    from conftest import request_context  # isort: skip

    deadline = asyncio.get_running_loop().time() + timeout_s
    found: dict[int, str] = {}
    while asyncio.get_running_loop().time() < deadline:
        reader = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            graph_name=graph,
        )
        try:
            rows = await reader.rdf.sparql(
                "SELECT ?revision ?body WHERE { ?node a <" + CORE + "PromptRevision> ; "
                "<" + CORE + "componentId> \"" + component_id + "\" ; "
                "<" + CORE + "entryRevision> ?revision ; "
                "<" + CORE + "bodySha256> ?body }"
            )
        except EngineResponseError:
            rows = []
        finally:
            await reader.close()
        found = {int(row["revision"]): str(row["body"]) for row in rows}
        if len(found) >= want:
            return found
        await asyncio.sleep(0.5)
    return found


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.8")
@pytest.mark.asyncio
async def test_prompt_graph_projection() -> None:
    client = await connect()
    component_id = f"prompt:graph-{uuid.uuid4().hex[:12]}"
    first = json.dumps({"task": "first", "schema_revision": 1}, separators=(",", ":"))
    second = json.dumps({"task": "second", "schema_revision": 1}, separators=(",", ":"))
    first_pin = hashlib.sha256(first.encode()).hexdigest()
    second_pin = hashlib.sha256(second.encode()).hexdigest()
    graph = _native_graph(TEST_TENANT)
    first_context = _publish_context(0)
    first_key = f"prompt-publish-{uuid.uuid4().hex}"
    try:
        await _publish_prompt(
            client,
            component_id,
            first,
            first_pin,
            0,
            context=first_context,
            idempotency_key=first_key,
        )
        # The graph names the committed identity: the revision and the body pin.
        projected = await _projected_revisions(graph, component_id, 1)
        assert projected == {1: first_pin}

        await _publish_prompt(client, component_id, second, second_pin, 1)
        projected = await _projected_revisions(graph, component_id, 2)
        assert projected == {1: first_pin, 2: second_pin}

        # Replaying the first publish returns the stored result and changes nothing:
        # no event re-projects an old revision over the newer one.
        await _publish_prompt(
            client,
            component_id,
            first,
            first_pin,
            0,
            context=first_context,
            idempotency_key=first_key,
        )
        replayed = await _projected_revisions(graph, component_id, 2)
        assert replayed == {1: first_pin, 2: second_pin}
        current = await send_agent_component_current(
            client,
            AgentComponentOpCurrent(
                op="current", tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        assert current is not None and current.entry_revision == 2
    finally:
        await client.close()
