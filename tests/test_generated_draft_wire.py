"""The generated draft models admit only what the engine decodes.

A client that validates against ``medulla.generated`` must not build a request the
engine refuses with ``invalid request encoding``: the Rust ``attributes`` field is a
non-optional map with a default, so the model carries no ``None`` for it, and its
encoding is a map whether the caller set it or not.
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated.agent_component import AgentComponentDraft

pytestmark = pytest.mark.no_engine

DIGEST = "sha256:" + "1" * 64


def _draft(**extra: object) -> dict[str, object]:
    return {
        "component_id": "prompt:example",
        "kind": "system_prompt",
        "version": "1.0.0",
        "content_digest": DIGEST,
        "facts": {
            "facts": "system_prompt",
            "prompt_mode": "static",
            "token_estimate": 1,
            "variables": [],
        },
        "provenance": {"origin": "native"},
        "summary": "A prompt.",
        "tenant_id": "tenant-a",
        "actor_scope": "agent-builder",
        "purpose_id": "agent-construction",
        "policy_digest": DIGEST,
        "source_revision": "rev-1",
        "source_revision_digest": DIGEST,
        **extra,
    }


# spec: MEDULLA-ORCH-VOCAB-R800
def test_attributes_default_to_an_empty_map_and_encode_as_one() -> None:
    draft = AgentComponentDraft.model_validate(_draft())
    assert draft.attributes == {}
    assert draft.model_dump(mode="json")["attributes"] == {}
    round_tripped = AgentComponentDraft.model_validate(draft.model_dump(mode="json"))
    assert round_tripped == draft


# spec: MEDULLA-ORCH-VOCAB-R800
def test_attributes_refuse_a_null_the_engine_would_have_refused() -> None:
    with pytest.raises(ValidationError):
        AgentComponentDraft.model_validate(_draft(attributes=None))
    kept = AgentComponentDraft.model_validate(_draft(attributes={"k": "v"}))
    assert kept.model_dump(mode="json")["attributes"] == {"k": "v"}


# spec: MEDULLA-ORCH-VOCAB-R810.1
def test_template_list_op_round_trips_its_wire_shape() -> None:
    from pydantic import TypeAdapter

    from medulla.generated.models import AgentTemplateOp, AgentTemplateOpList

    wire = {
        "op": "list",
        "request": {"tenant_id": "tenant-a", "limit": 10, "cursor": "abc"},
    }
    op = TypeAdapter(AgentTemplateOp).validate_python(wire)
    assert isinstance(op, AgentTemplateOpList)
    assert op.model_dump(mode="json") == wire
