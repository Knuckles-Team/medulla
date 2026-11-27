"""MEDULLA-DECISION-ENGINE-R832: the ``credential_reuse`` Decide request shape."""

from __future__ import annotations

import json

import pytest

from medulla import decision_client as dc

_USAGE = {
    "owner": "usr:alice",
    "agent_id": "agent-worker",
    "connector": "github-mcp",
    "provider": "github",
    "purpose": "triage",
    "scopes": ["repo:read"],
}
_BINDING = {
    "owner": "usr:alice",
    "agents": ["agent-worker"],
    "connectors": ["github-mcp"],
    "providers": ["github"],
    "scopes": ["repo:read"],
    "purposes": ["triage"],
    "expires_at_ms": 10,
    "revoked": False,
    "revision": 1,
}


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R832")
def test_credential_reuse_request_shape() -> None:
    request = dc.credential_reuse_request(
        "tenant:t", "q1", _USAGE, {"schema": "s"}, binding=_BINDING
    )
    assert request["question"] == {
        "question_id": "q1",
        "kind": "credential_reuse",
        "safety": "ordinary",
    }
    params = {p["name"]: p["value"] for p in request["params"]}
    assert params["owner"] == {"type": "text", "value": "usr:alice"}
    assert json.loads(params["scopes"]["value"]) == ["repo:read"]
    assert json.loads(params["binding"]["value"])["revision"] == 1
    # The one declared candidate is the admit option; the secret is never a param.
    assert [o["option_id"] for o in request["candidates"]["options"]] == ["admit"]
    assert "secret" not in params


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R832")
def test_credential_reuse_request_without_binding_sends_none() -> None:
    request = dc.credential_reuse_request("tenant:t", "q1", _USAGE, {"schema": "s"})
    assert "binding" not in {p["name"] for p in request["params"]}
