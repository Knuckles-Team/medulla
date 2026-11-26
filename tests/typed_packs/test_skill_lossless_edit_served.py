"""MEDULLA-TYPED-PACKS-R100.100 served acceptance: lossless SKILL.md editing.

``AgentComponent.edit_skill_source`` patches the original SKILL.md bytes by byte range and
publishes the result as the next revision. Nothing is reparsed or reserialized, so every
untouched byte (custom fields, comments, spacing, order, CRLF style) stays identical. A
stale digest, overlapping patches, an unknown span or a split character refuses and writes
nothing.
"""
# ruff: noqa: E501

from __future__ import annotations

import hashlib
import uuid
from typing import Any

import pytest
from ontology_pack_support import connect

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated._shared import AgentLibraryMutationContext
from medulla.generated.agent_component import (
    AgentComponentContentRequest,
    AgentComponentOpCurrent,
    SkillSourceEditRequest,
    SkillSourcePatch,
)
from medulla.generated.storage import (
    send_agent_component,
    send_agent_component_content,
    send_agent_component_current,
    send_agent_component_edit_skill_source,
)

from conftest import TEST_TENANT  # isort: skip

SOURCE = (
    "---\r\n"
    "name: lossless-demo\r\n"
    "description: Original description\r\n"
    "# a comment: kept as written\r\n"
    "x-custom:   spaced   value\r\n"
    "---\r\n"
    "## Steps\r\n"
    "1. First step\r\n"
    "2. Second step\r\n"
    "3. Café step\r\n"
).encode()


def _digest(body: bytes) -> str:
    return "sha256:" + hashlib.sha256(body).hexdigest()


def _context(expected_revision: int) -> dict[str, Any]:
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


def _skill_draft(component_id: str, body: bytes) -> dict[str, Any]:
    return {
        "component_id": component_id,
        "kind": "skill",
        "version": "1.0.0",
        "content_digest": _digest(body),
        "content_ref": None,
        "facts": {"facts": "opaque"},
        "provenance": {"origin": "native"},
        "summary": "natively published skill",
        "attributes": {},
        "tenant_id": TEST_TENANT,
        "actor_scope": "placeholder",
        "purpose_id": "placeholder",
        "policy_digest": "sha256:" + "0" * 64,
        "source_revision": "rev-1",
        "source_revision_digest": "sha256:" + "8" * 64,
    }


async def _publish_skill(client: MedullaClient, component_id: str, body: bytes) -> Any:
    request = {
        "context": _context(0),
        "component": _skill_draft(component_id, body),
        "evaluation_receipt_digest": None,
        "body": {"kind": "skill_source", "text": body.decode()},
    }
    return await send_agent_component(
        client,
        {"op": {"op": "publish", "request": request}},
        idempotency_key=f"skill-publish-{uuid.uuid4().hex}",
    )


def _patch(source: bytes, anchor: str, replacement: str) -> SkillSourcePatch:
    start = source.index(anchor.encode())
    return SkillSourcePatch(
        range_start=start,
        range_end=start + len(anchor.encode()),
        replacement=replacement,
    )


async def _edit(
    client: MedullaClient,
    component_id: str,
    patches: list[SkillSourcePatch],
    *,
    source_digest: str | None = None,
    expected_revision: int = 1,
) -> Any:
    return await send_agent_component_edit_skill_source(
        client,
        SkillSourceEditRequest(
            context=AgentLibraryMutationContext.model_validate(
                _context(expected_revision)
            ),
            component_id=component_id,
            source_digest=source_digest or _digest(SOURCE),
            patches=patches,
        ),
        idempotency_key=f"skill-edit-{uuid.uuid4().hex}",
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R100.100")
@pytest.mark.asyncio
async def test_frontmatter_and_step_edits_preserve_unmodified_bytes() -> None:
    component_id = f"skill:lossless-{uuid.uuid4().hex[:10]}"
    client = await connect()
    try:
        await _publish_skill(client, component_id, SOURCE)
        patches = [
            _patch(SOURCE, "Original description", "Edited description"),
            _patch(SOURCE, "Second step", "Revised step"),
        ]
        committed = await _edit(client, component_id, patches)
        expected = SOURCE.replace(
            b"Original description", b"Edited description"
        ).replace(b"Second step", b"Revised step")
        assert committed.component.entry_revision == 2
        assert committed.component.content_digest == _digest(expected)

        content = await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        # Every byte outside the two patched ranges is identical: the comment, the custom
        # field with its odd spacing, the order and the CRLF line endings.
        assert content.body == expected
        assert (
            b"# a comment: kept as written\r\nx-custom:   spaced   value\r\n"
            in content.body
        )
        assert content.body.count(b"\r\n") == SOURCE.count(b"\r\n")
        assert content.content_digest == _digest(expected)
        # The first revision keeps its own bytes.
        first = await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id=TEST_TENANT, component_id=component_id, entry_revision=1
            ),
        )
        assert first.body == SOURCE
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R100.100")
@pytest.mark.asyncio
async def test_stale_or_ambiguous_edit_refuses_before_write() -> None:
    component_id = f"skill:lossless-{uuid.uuid4().hex[:10]}"
    client = await connect()
    try:
        await _publish_skill(client, component_id, SOURCE)
        step = _patch(SOURCE, "First step", "Changed")

        with pytest.raises(EngineResponseError, match="SKILL_SOURCE_STALE"):
            await _edit(client, component_id, [step], source_digest=_digest(b"other"))

        overlapping = [
            _patch(SOURCE, "Original description", "A"),
            SkillSourcePatch(
                range_start=SOURCE.index(b"Original") + 3,
                range_end=SOURCE.index(b"Original") + 6,
                replacement="B",
            ),
        ]
        with pytest.raises(EngineResponseError, match="SKILL_PATCH_OVERLAP"):
            await _edit(client, component_id, overlapping)

        with pytest.raises(EngineResponseError, match="SKILL_SPAN_UNKNOWN"):
            await _edit(
                client,
                component_id,
                [
                    SkillSourcePatch(
                        range_start=len(SOURCE) + 5,
                        range_end=len(SOURCE) + 9,
                        replacement="x",
                    )
                ],
            )

        # A boundary inside the two bytes of the accented letter splits a character.
        inside = SOURCE.index("é".encode()) + 1
        with pytest.raises(EngineResponseError, match="SKILL_PATCH_ANCHOR_INVALID"):
            await _edit(
                client,
                component_id,
                [
                    SkillSourcePatch(
                        range_start=inside, range_end=inside, replacement="x"
                    )
                ],
            )

        # A bare line feed breaks the CRLF style of the source.
        with pytest.raises(EngineResponseError, match="SKILL_PATCH_ANCHOR_INVALID"):
            await _edit(
                client,
                component_id,
                [_patch(SOURCE, "First step", "two\nlines")],
            )

        # An edit that breaks the front matter is refused by the SKILL.md checks.
        with pytest.raises(EngineResponseError, match="MALFORMED_BODY"):
            await _edit(
                client,
                component_id,
                [_patch(SOURCE, "name: lossless-demo", "name: renamed")],
            )

        # No refusal wrote a revision, and the head still holds the original bytes.
        assert await _head(client, component_id) == 1
        content = await send_agent_component_content(
            client,
            AgentComponentContentRequest(
                tenant_id=TEST_TENANT, component_id=component_id
            ),
        )
        assert content.body == SOURCE
    finally:
        await client.close()
