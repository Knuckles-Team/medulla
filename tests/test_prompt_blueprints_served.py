"""Served prompt-blueprint acceptance (MEDULLA-TYPED-PACKS-R094.x).

A packaged prompt is exported as a native blueprint: the complete admitted
record at one pinned revision plus its body bytes, which travel verified and
never parsed. Each test imports a prompt through the served connector-pack
import and reads the blueprint back over the wire.
"""
# ruff: noqa: E501, I001

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
from pydantic import TypeAdapter

sys.path.insert(0, str(Path(__file__).resolve().parent / "typed_packs"))

from _commons_generation import connect as connect_engine, restartable_engine
from ontology_pack_support import connect, import_entries

from conftest import TEST_TENANT, request_context, tenant_request_context  # isort: skip
from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.agent_component import (
    AgentComponentBlueprintExportRequest,
    AgentComponentContentRequest,
    AgentComponentEntry,
    AgentComponentOpCurrent,
    AgentComponentOpRecordSourceEffect,
    AgentComponentOpVerifyRender,
    BlueprintImportRequest,
    BlueprintProposalRequest,
    PromptRenderRequest,
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
    send_agent_component_import_blueprint,
    send_agent_component_record_source_effect,
    send_agent_component_render,
    send_agent_component_store_proposal,
    send_agent_component_verify_render,
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
    graph: str,
    component_id: str,
    want: int,
    timeout_s: float = 120.0,
    socket_path: str | None = None,
    auth_secret: str | None = None,
) -> dict[int, str]:
    """The ``PromptRevision`` nodes of one component by entry revision, once ``want`` exist.

    ``socket_path`` (default: the session engine) names the engine to read, and
    ``auth_secret`` the secret a restarted engine signs with.
    """
    deadline = asyncio.get_running_loop().time() + timeout_s
    found: dict[int, str] = {}
    while asyncio.get_running_loop().time() < deadline:
        secret: dict[str, Any] = (
            {} if auth_secret is None else {"auth_secret": auth_secret}
        )
        reader = await MedullaClient.connect(
            socket_path=socket_path or os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            graph_name=graph,
            **secret,
        )
        try:
            rows = await reader.rdf.sparql(
                "SELECT ?revision ?body WHERE { ?node a <" + CORE + "PromptRevision> ; "
                "<" + CORE + 'componentId> "' + component_id + '" ; '
                "<" + CORE + "entryRevision> ?revision ; "
                "<" + CORE + "bodySha256> ?body }"
            )
        except EngineResponseError:
            rows = []
        finally:
            await reader.close()
        found = {int(str(row["revision"])): str(row["body"]) for row in rows}
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


def _canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def _frame_pins(tenant: str, component_id: str, revision: int, pin: str) -> bytes:
    """Each pin as a big-endian u32 length and its bytes, in the engine's fixed order."""
    fields = [
        tenant.encode(),
        component_id.encode(),
        struct.pack(">Q", revision),
        struct.pack(">I", 1),
        pin.encode(),
    ]
    return b"".join(struct.pack(">I", len(field)) + field for field in fields)


async def _render(
    client: MedullaClient,
    component_id: str,
    variables: dict[str, Any],
    *,
    revision: int = 1,
    schema_revision: int = 1,
    tenant: str = TEST_TENANT,
) -> Any:
    return await send_agent_component_render(
        client,
        PromptRenderRequest(
            tenant_id=tenant,
            component_id=component_id,
            revision=revision,
            schema_revision=schema_revision,
            variables=variables,
        ),
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.9")
@pytest.mark.asyncio
async def test_prompt_render_route() -> None:
    client = await connect()
    component_id = f"prompt:render-{uuid.uuid4().hex[:12]}"
    document = {"task": "greet", "schema_revision": 1, "variables": ["audience"]}
    blueprint = json.dumps(document, sort_keys=True, separators=(",", ":"))
    pin = hashlib.sha256(blueprint.encode()).hexdigest()
    variables = {"audience": "ops"}
    try:
        await _publish_prompt(client, component_id, blueprint, pin, 0)
        result = await _render(client, component_id, variables)

        # The result is the pinned revision and the exact rendered bytes.
        pins = _frame_pins(TEST_TENANT, component_id, 1, pin)
        expected = _canonical(
            {
                "rendering_version": 1,
                "pins": hashlib.sha256(pins).hexdigest(),
                "blueprint": document,
                "variables": variables,
            }
        )
        assert result.schema_version == 1
        assert result.bytes == expected
        assert result.render_digest == hashlib.sha256(expected).hexdigest()
        identity = result.receipt.identity
        assert identity.tenant_id == TEST_TENANT
        assert identity.component_id == component_id
        assert identity.entry_revision == 1
        assert identity.schema_revision == 1
        assert identity.body_sha256 == pin

        # The receipt binds the framed pins, the variables digest and the scope.
        variables_digest = hashlib.sha256(_canonical(variables)).hexdigest()
        assert result.receipt.variables_digest == variables_digest
        assert result.receipt.scope == TEST_TENANT
        framed = pins
        for field in (variables_digest.encode(), TEST_TENANT.encode()):
            framed += struct.pack(">I", len(field)) + field
        assert result.receipt.receipt_digest == hashlib.sha256(framed).hexdigest()

        # A repeat render is byte-identical; a changed variable changes the digest.
        again = await _render(client, component_id, variables)
        assert again.bytes == result.bytes
        other = await _render(client, component_id, {"audience": "dev"})
        assert other.render_digest != result.render_digest

        # Each invalid request is refused by name.
        with pytest.raises(EngineResponseError, match="PROMPT_VARIABLE_MISSING"):
            await _render(client, component_id, {})
        with pytest.raises(EngineResponseError, match="PROMPT_VARIABLE_UNKNOWN"):
            await _render(client, component_id, {**variables, "extra": 1})
        with pytest.raises(EngineResponseError, match="PROMPT_SCHEMA_INVALID"):
            await _render(client, component_id, variables, schema_revision=2)
        with pytest.raises(EngineResponseError, match="COMPONENT_REVISION_UNKNOWN"):
            await _render(client, component_id, variables, revision=99)
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _render(client, component_id, variables, tenant="tenant-other")

        # Rendering writes nothing: the head and the body are unchanged.
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
    finally:
        await client.close()


async def _verify(client: MedullaClient, receipt: Any) -> Any:
    return await send_agent_component_verify_render(
        client,
        AgentComponentOpVerifyRender(op="verify_render", receipt=receipt),
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.10")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.13")
@pytest.mark.asyncio
async def test_receipt_verdicts() -> None:
    client = await connect()
    component_id = f"prompt:verify-{uuid.uuid4().hex[:12]}"
    first = json.dumps({"task": "one", "schema_revision": 1}, separators=(",", ":"))
    second = json.dumps({"task": "two", "schema_revision": 1}, separators=(",", ":"))
    first_pin = hashlib.sha256(first.encode()).hexdigest()
    second_pin = hashlib.sha256(second.encode()).hexdigest()
    try:
        await _publish_prompt(client, component_id, first, first_pin, 0)
        rendered = await _render(client, component_id, {})
        receipt = rendered.receipt

        # A fresh receipt for the live head verifies.
        assert (await _verify(client, receipt)).ok is True

        # A changed digest, variables digest or scope refuses by name.
        forged = receipt.model_copy(update={"receipt_digest": "0" * 64})
        with pytest.raises(EngineResponseError, match="PROMPT_RECEIPT_INVALID"):
            await _verify(client, forged)
        changed = receipt.model_copy(update={"variables_digest": "1" * 64})
        with pytest.raises(EngineResponseError, match="PROMPT_RECEIPT_INVALID"):
            await _verify(client, changed)
        rescoped = receipt.model_copy(update={"scope": "tenant-other"})
        with pytest.raises(EngineResponseError, match="PROMPT_RECEIPT_INVALID"):
            await _verify(client, rescoped)

        # A receipt for another tenant is denied before any read.
        foreign_identity = receipt.identity.model_copy(
            update={"tenant_id": "tenant-other"}
        )
        foreign = receipt.model_copy(update={"identity": foreign_identity})
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _verify(client, foreign)

        # Once a newer revision is the head, the earlier receipt no longer verifies.
        await _publish_prompt(client, component_id, second, second_pin, 1)
        with pytest.raises(EngineResponseError, match="PROMPT_RECEIPT_INVALID"):
            await _verify(client, receipt)
        latest = await _render(client, component_id, {}, revision=2)
        assert (await _verify(client, latest.receipt)).ok is True
    finally:
        await client.close()


# Entry fields the server owns: import ignores them and publish restamps them.
_SERVER_OWNED = {
    "schema_version",
    "lifecycle",
    "definition_digest",
    "created_at_ms",
    "updated_at_ms",
    "actor_scope",
    "purpose_id",
    "policy_digest",
    "content_ref",
    "entry_revision",
}


def _edited_envelope(
    exported: Any, body: bytes, **entry_changes: Any
) -> dict[str, Any]:
    """The export envelope with `body` and the pins that follow it applied."""
    pin = hashlib.sha256(body).hexdigest()
    envelope = exported.model_dump(mode="json")
    envelope["body"] = list(body)
    envelope["content_digest"] = "sha256:" + pin
    envelope["entry"]["content_digest"] = "sha256:" + pin
    envelope["entry"]["facts"]["body_sha256"] = pin
    envelope["entry"].update(entry_changes)
    return envelope


async def _import_blueprint(
    client: MedullaClient,
    envelope: dict[str, Any],
    expected_revision: int,
    tenant: str = TEST_TENANT,
    idempotency_key: str | None = None,
) -> Any:
    context = _publish_context(expected_revision)
    context["tenant_id"] = tenant
    envelope["entry"]["tenant_id"] = tenant
    return await send_agent_component_import_blueprint(
        client,
        BlueprintImportRequest.model_validate(
            {"context": context, "envelope": envelope}
        ),
        idempotency_key=idempotency_key or f"blueprint-import-{uuid.uuid4().hex}",
    )


async def _head(client: MedullaClient, component_id: str) -> int:
    current = await send_agent_component_current(
        client,
        AgentComponentOpCurrent(
            op="current", tenant_id=TEST_TENANT, component_id=component_id
        ),
    )
    assert current is not None
    return current.entry_revision


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.3")
@pytest.mark.asyncio
async def test_modified_blueprint_import() -> None:
    client = await connect()
    component_id = f"prompt:native-{uuid.uuid4().hex[:12]}"
    original = json.dumps(
        {"task": "summarise the ticket", "schema_revision": 1},
        sort_keys=True,
        separators=(",", ":"),
    )
    edited = original.replace("summarise", "classify").encode()
    try:
        await _publish_prompt(
            client,
            component_id,
            original,
            hashlib.sha256(original.encode()).hexdigest(),
            0,
        )
        first = await _export(client, component_id, 1)

        # The edited envelope imports at the base revision as exactly one new revision.
        committed = await _import_blueprint(
            client, _edited_envelope(first, edited), expected_revision=1
        )
        assert committed.component.entry_revision == 2
        second = await _export(client, component_id, 2)
        assert second.body == edited
        assert second.content_digest == "sha256:" + hashlib.sha256(edited).hexdigest()
        before = first.entry.model_dump(mode="json")
        after = second.entry.model_dump(mode="json")
        changed = {
            key
            for key in before
            if key not in _SERVER_OWNED and before[key] != after[key]
        }
        assert changed == {"content_digest", "facts"}, changed
        assert {
            key: value for key, value in before["facts"].items() if key != "body_sha256"
        } == {
            key: value for key, value in after["facts"].items() if key != "body_sha256"
        }

        # Every refusal leaves the head at 2.
        with pytest.raises(EngineResponseError, match="STALE_AGENT_LIBRARY_REVISION"):
            await _import_blueprint(
                client, _edited_envelope(first, edited), expected_revision=1
            )
        for change in (
            {"kind": "tool"},
            {
                "provenance": {
                    "origin": "source_package",
                    "package_id": "other",
                    "package_version": "1",
                }
            },
        ):
            with pytest.raises(EngineResponseError, match="BLUEPRINT_IDENTITY_CHANGED"):
                await _import_blueprint(
                    client,
                    _edited_envelope(first, edited, **change),
                    expected_revision=1,
                )
        disagreeing = _edited_envelope(first, edited)
        disagreeing["content_digest"] = "sha256:" + "0" * 64
        with pytest.raises(EngineResponseError, match="BLUEPRINT_MALFORMED"):
            await _import_blueprint(client, disagreeing, expected_revision=1)
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _import_blueprint(
                client,
                _edited_envelope(first, edited),
                expected_revision=1,
                tenant="tenant-other",
            )
        assert await _head(client, component_id) == 2

        # A pack-imported prompt is pack-owned: it changes only through pack import.
        connector = f"blueprint-{uuid.uuid4().hex[:10]}"
        await _import_prompt(client, connector, "owned")
        owned_id = _component_id(connector, "owned")
        owned = await _export(client, owned_id, 1)
        with pytest.raises(EngineResponseError, match="RESERVED_COMPONENT_ID"):
            packaged = owned.model_dump(mode="json")
            packaged["body"] = list(owned.body)
            await _import_blueprint(client, packaged, expected_revision=1)
        assert await _head(client, owned_id) == 1
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.4")
@pytest.mark.asyncio
async def test_blueprint_retry_identity() -> None:
    client = await connect()
    component_id = f"prompt:native-{uuid.uuid4().hex[:12]}"
    original = json.dumps(
        {"task": "summarise the ticket", "schema_revision": 1},
        sort_keys=True,
        separators=(",", ":"),
    )
    edited = original.replace("summarise", "classify").encode()
    other = original.replace("summarise", "route").encode()
    key = f"blueprint-retry-{uuid.uuid4().hex}"
    try:
        await _publish_prompt(
            client,
            component_id,
            original,
            hashlib.sha256(original.encode()).hexdigest(),
            0,
        )
        first = await _export(client, component_id, 1)
        committed = await _import_blueprint(
            client,
            _edited_envelope(first, edited),
            expected_revision=1,
            idempotency_key=key,
        )
        assert committed.component.entry_revision == 2
        content_before = await _content(client, component_id, 2)

        # The same request under the same key, with a fresh attempt nonce, replays
        # the recorded receipt and creates no revision.
        replayed = await _import_blueprint(
            client,
            _edited_envelope(first, edited),
            expected_revision=1,
            idempotency_key=key,
        )
        assert replayed.component.entry_revision == committed.component.entry_revision
        assert (
            replayed.component.definition_digest
            == committed.component.definition_digest
        )
        history = await _history(client, component_id)
        assert sorted(entry.entry_revision for entry in history) == [1, 2]
        assert await _content(client, component_id, 2) == content_before

        # The same key with different content is an idempotency conflict.
        with pytest.raises(EngineResponseError, match="IDEMPOTENCY_CONFLICT"):
            await _import_blueprint(
                client,
                _edited_envelope(first, other),
                expected_revision=1,
                idempotency_key=key,
            )
        assert await _head(client, component_id) == 2
        assert await _content(client, component_id, 2) == content_before
    finally:
        await client.close()


async def _history(client: MedullaClient, component_id: str) -> list[Any]:
    return TypeAdapter(list[AgentComponentEntry]).validate_python(
        await client._send(
            "AgentComponent",
            {
                "op": {
                    "op": "history",
                    "tenant_id": TEST_TENANT,
                    "component_id": component_id,
                }
            },
            None,
        )
    )


async def _content(client: MedullaClient, component_id: str, revision: int) -> bytes:
    content = await send_agent_component_content(
        client,
        AgentComponentContentRequest(
            tenant_id=TEST_TENANT, component_id=component_id, entry_revision=revision
        ),
    )
    return bytes(content.body)


_NATIVE_LOSSLESS = (
    Path(__file__).parent / "fixtures" / "blueprint" / "native_lossless.json"
)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.5")
@pytest.mark.no_engine
def test_blueprint_roundtrip_restart(tmp_path: Path) -> None:
    component_id = f"prompt:roundtrip-{uuid.uuid4().hex[:12]}"
    fixture = _NATIVE_LOSSLESS.read_bytes()
    pin = hashlib.sha256(fixture).hexdigest()
    assert fixture.count(b'"summarise"') == 1
    edited = fixture.replace(b'"summarise"', b'"summarise-edited"')
    context_b = tenant_request_context("tenant-restart-b")

    def comparable(exported: Any) -> dict[str, Any]:
        envelope = exported.model_dump(mode="json")
        for key in _SERVER_OWNED:
            envelope["entry"].pop(key, None)
        for key in ("content_digest", "body"):
            envelope.pop(key)
        envelope["entry"].pop("content_digest")
        envelope["entry"]["facts"].pop("body_sha256")
        return envelope

    async def run_one(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        try:
            await _publish_prompt(client, component_id, fixture.decode(), pin, 0)
            first = await _export(client, component_id, 1)
            assert bytes(first.body) == fixture
            committed = await _import_blueprint(
                client, _edited_envelope(first, edited), expected_revision=1
            )
            assert committed.component.entry_revision == 2
        finally:
            await client.close()

    async def run_two(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        other = await connect_engine(engine=engine, context=context_b)
        try:
            second_bytes = await _content(client, component_id, 2)
            first_bytes = await _content(client, component_id, 1)
            assert second_bytes == edited
            assert first_bytes == fixture
            span = len(b"-edited")
            prefix = 0
            while fixture[prefix] == edited[prefix]:
                prefix += 1
            suffix = 0
            while fixture[-1 - suffix] == edited[-1 - suffix]:
                suffix += 1
            assert (
                prefix + suffix == len(fixture) and len(edited) - len(fixture) == span
            )
            first = await _export(client, component_id, 1)
            second = await _export(client, component_id, 2)
            assert comparable(first) == comparable(second)

            # A stale import and a cross-tenant import refuse and change nothing.
            with pytest.raises(
                EngineResponseError, match="STALE_AGENT_LIBRARY_REVISION"
            ):
                await _import_blueprint(
                    client, _edited_envelope(first, edited), expected_revision=1
                )
            with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
                await _import_blueprint(
                    other, _edited_envelope(first, edited), expected_revision=1
                )
            history = await _history(client, component_id)
            assert sorted(entry.entry_revision for entry in history) == [1, 2]
            assert await _head(client, component_id) == 2
        finally:
            await other.close()
            await client.close()

    with restartable_engine(tmp_path, "run1", bootstrap=True) as engine:
        asyncio.run(run_one(engine))
    with restartable_engine(tmp_path, "run2", bootstrap=False) as engine:
        asyncio.run(run_two(engine))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R094.6")
@pytest.mark.asyncio
async def test_prompt_proposal_has_no_authority() -> None:
    client = await connect()
    component_id = f"prompt:native-{uuid.uuid4().hex[:12]}"
    original = json.dumps(
        {"task": "summarise the ticket", "schema_revision": 1},
        sort_keys=True,
        separators=(",", ":"),
    )
    proposed = original.replace("summarise", "classify").encode()
    payload_digest = "sha256:" + hashlib.sha256(proposed).hexdigest()

    def request(**proposal_changes: Any) -> BlueprintProposalRequest:
        proposal = {
            "source_revision": 1,
            "payload_digest": payload_digest,
            "comparison": "classify scored 0.82 against 0.78 on the held-out set",
            **proposal_changes,
        }
        return BlueprintProposalRequest.model_validate(
            {
                "context": _publish_context(1),
                "component_id": component_id,
                "proposal": proposal,
            }
        )

    try:
        await _publish_prompt(
            client,
            component_id,
            original,
            hashlib.sha256(original.encode()).hexdigest(),
            0,
        )
        before = await _content(client, component_id, 1)
        key = f"blueprint-proposal-{uuid.uuid4().hex}"

        # The proposal is recorded; the same proposal again writes nothing.
        stored = await send_agent_component_store_proposal(
            client, request(), idempotency_key=key
        )
        assert stored.recorded is True
        assert stored.source_revision == 1
        assert stored.payload_digest == payload_digest
        again = await send_agent_component_store_proposal(
            client, request(), idempotency_key=key
        )
        assert again.recorded is False

        # It carries no authority: the head, the history and the body are unchanged.
        assert await _head(client, component_id) == 1
        history = await _history(client, component_id)
        assert [entry.entry_revision for entry in history] == [1]
        assert await _content(client, component_id, 1) == before

        # A proposal that supplies its own approval, actor scope or permission is refused.
        for field, value in (
            ("approval", "approved"),
            ("actor_scope", "admin"),
            ("permission", "agent:component-write"),
        ):
            with pytest.raises(EngineResponseError, match="PROPOSAL_SELF_APPROVED"):
                await send_agent_component_store_proposal(
                    client,
                    request(**{field: value}),
                    idempotency_key=f"blueprint-proposal-{uuid.uuid4().hex}",
                )
        # A proposal against a revision that does not exist is refused.
        with pytest.raises(EngineResponseError, match="COMPONENT_REVISION_UNKNOWN"):
            await send_agent_component_store_proposal(
                client,
                request(source_revision=9),
                idempotency_key=f"blueprint-proposal-{uuid.uuid4().hex}",
            )
        assert await _head(client, component_id) == 1
    finally:
        await client.close()


async def _refused_for_tenant_b(
    client: MedullaClient, component_id: str, secrets: list[str]
) -> None:
    """Tenant B reads nothing of tenant A: ``ACCESS_DENIED`` naming A, and an unknown
    revision naming B; neither error text carries a blueprint byte or the summary."""
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED") as denied:
        await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
    with pytest.raises(
        EngineResponseError, match="COMPONENT_REVISION_UNKNOWN"
    ) as unknown:
        await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id="tenant-restart-b",
                component_id=component_id,
                entry_revision=1,
            ),
        )
    for error in (denied, unknown):
        for secret in secrets:
            assert secret not in str(error.value)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R002.11")
@pytest.mark.no_engine
def test_prompt_restart_readback(tmp_path: Path) -> None:
    component_id = f"prompt:restart-{uuid.uuid4().hex[:12]}"
    first = json.dumps(
        {"task": "restart one", "schema_revision": 1}, separators=(",", ":")
    )
    second = json.dumps(
        {"task": "restart two", "schema_revision": 1}, separators=(",", ":")
    )
    pins = {
        1: hashlib.sha256(first.encode()).hexdigest(),
        2: hashlib.sha256(second.encode()).hexdigest(),
    }
    graph = _native_graph(TEST_TENANT)
    leaks = [first, second, "natively published prompt"]
    context_b = tenant_request_context("tenant-restart-b")

    async def head(client: MedullaClient) -> int:
        return await _head(client, component_id)

    async def stale_refused(client: MedullaClient) -> None:
        with pytest.raises(EngineResponseError, match="STALE_AGENT_LIBRARY_REVISION"):
            await _publish_prompt(client, component_id, first, pins[1], 0)
        assert await head(client) == 2

    async def run_one(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        other = await connect_engine(engine=engine, context=context_b)
        try:
            await _publish_prompt(client, component_id, first, pins[1], 0)
            await _publish_prompt(client, component_id, second, pins[2], 1)
            projected = await _projected_revisions(
                graph,
                component_id,
                2,
                socket_path=engine.socket,
                auth_secret=engine.secret,
            )
            assert projected == pins
            await stale_refused(client)
            await _refused_for_tenant_b(other, component_id, leaks)
        finally:
            await other.close()
            await client.close()

    async def run_two(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        other = await connect_engine(engine=engine, context=context_b)
        try:
            history = TypeAdapter(list[AgentComponentEntry]).validate_python(
                await client._send(
                    "AgentComponent",
                    {
                        "op": {
                            "op": "history",
                            "tenant_id": TEST_TENANT,
                            "component_id": component_id,
                        }
                    },
                    None,
                )
            )
            assert sorted(entry.entry_revision for entry in history) == [1, 2]
            assert await head(client) == 2
            for revision, body in ((1, first), (2, second)):
                content = await send_agent_component_content(
                    client,
                    AgentComponentContentRequest(
                        tenant_id=TEST_TENANT,
                        component_id=component_id,
                        entry_revision=revision,
                    ),
                )
                assert content.body == body.encode()
                assert content.content_digest == "sha256:" + pins[revision]
            projected = await _projected_revisions(
                graph,
                component_id,
                2,
                socket_path=engine.socket,
                auth_secret=engine.secret,
            )
            assert projected == pins
            await stale_refused(client)
            await _refused_for_tenant_b(other, component_id, leaks)
        finally:
            await other.close()
            await client.close()

    with restartable_engine(tmp_path, "run1", bootstrap=True) as engine:
        asyncio.run(run_one(engine))
    with restartable_engine(tmp_path, "run2", bootstrap=False) as engine:
        asyncio.run(run_two(engine))
