"""Served agent knowledge scope on Discover (MEDULLA-FEDERATED-QUERY-R704.3).

Two agents read one corpus. Each agent definition node declares its `knowledgeScope`; a
`Discover` that names the agent returns only what its scope admits, and an agent with no
scope sees everything the caller may read.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import time
import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put
from conftest import TEST_AGENT_ID, register_commons_agents, request_context


def _answer(cl: Any, graph: str, agent_ref: str | None) -> dict[str, Any]:
    body: dict[str, Any] = {
        "keywords": ["runbook"],
        "query_embedding": [],
        "k": 5,
    }
    if agent_ref is not None:
        body["agent_ref"] = agent_ref
    result = cl._send("Discover", body, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _discover(cl: Any, graph: str, agent_ref: str | None) -> list[str]:
    return sorted(hit["id"] for hit in _answer(cl, graph, agent_ref)["hits"])


def _digest(scope: dict[str, Any]) -> str:
    record: dict[str, Any] = {
        "container_refs": sorted(set(scope.get("container_refs", []))),
        "knowledge_sets": sorted(set(scope.get("knowledge_sets", []))),
        "source_kinds": sorted(set(scope.get("source_kinds", []))),
    }
    if "time_floor" in scope:
        record["time_floor"] = scope["time_floor"]
    text = json.dumps(record, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode()).hexdigest()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R704.3")
def test_agent_sees_only_scoped_sources(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R704.3: two agents over one corpus each read
    only what their definition node's knowledge scope admits."""
    graph = f"knowledge-scope-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        for node_id, kind in (
            ("doc-wiki", "confluence"),
            ("doc-chat", "slack"),
            ("doc-ticket", "jira"),
        ):
            put(
                cl,
                graph,
                node_id,
                {
                    "type": "Document",
                    "name": f"runbook {kind}",
                    "description": "runbook",
                    "source_kind": kind,
                },
            )
        put(
            cl,
            graph,
            "agent:wiki",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"source_kinds": ["confluence"]},
            },
        )
        put(
            cl,
            graph,
            "agent:chat",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"source_kinds": ["slack", "jira"]},
            },
        )
        put(cl, graph, "agent:open", {"type": "AgentDefinition"})

        assert _discover(cl, graph, "agent:wiki") == ["doc-wiki"]
        assert _discover(cl, graph, "agent:chat") == ["doc-chat", "doc-ticket"]
        # No scope, or no agent, is the caller's access alone.
        everything = ["doc-chat", "doc-ticket", "doc-wiki"]
        assert _discover(cl, graph, "agent:open") == everything
        assert _discover(cl, graph, None) == everything
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _corpus(cl: Any, graph: str) -> None:
    for node_id, kind in (("doc-wiki", "confluence"), ("doc-chat", "slack")):
        put(
            cl,
            graph,
            node_id,
            {
                "type": "Document",
                "name": "runbook",
                "description": "runbook",
                "source_kind": kind,
            },
        )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R704.4")
def test_scope_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R704.4: an agent with no definition node and a
    scope naming a foreign knowledge set each refuse, and a scoped answer is a subset of
    the unscoped one."""
    graph = f"knowledge-scope-refuse-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _corpus(cl, graph)
        # The write admits only sets the graph holds (R808); the set is removed after,
        # so the read still refuses a scope whose set no node of the view holds.
        put(cl, graph, "ks:theirs", {"type": "KnowledgeSet"})
        put(
            cl,
            graph,
            "agent:foreign",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"knowledge_sets": ["ks:theirs"]},
            },
        )
        cl._send("RemoveNode", {"node_id": "ks:theirs"}, graph)
        put(
            cl,
            graph,
            "agent:wiki",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"source_kinds": ["confluence"]},
            },
        )
        with pytest.raises(Exception, match="SCOPE_AGENT_UNKNOWN"):
            _discover(cl, graph, "agent:nobody")
        with pytest.raises(Exception, match="SCOPE_SET_UNKNOWN"):
            _discover(cl, graph, "agent:foreign")
        scoped = set(_discover(cl, graph, "agent:wiki"))
        assert scoped and scoped <= set(_discover(cl, graph, None))
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R704.5")
def test_scope_digest_recorded(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R704.5: each answer carries the digest of the
    applied scope, and an empty scope has the digest of the empty record."""
    graph = f"knowledge-scope-digest-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _corpus(cl, graph)
        scope = {"source_kinds": ["slack", "confluence"], "time_floor": 5}
        put(
            cl,
            graph,
            "agent:both",
            {"type": "AgentDefinition", "knowledgeScope": scope},
        )
        put(cl, graph, "agent:open", {"type": "AgentDefinition"})
        assert _answer(cl, graph, "agent:both")["scope_digest"] == _digest(scope)
        empty = _digest({})
        assert _answer(cl, graph, "agent:open")["scope_digest"] == empty
        assert _answer(cl, graph, None)["scope_digest"] == empty
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _framed(domain: bytes, fields: list[bytes]) -> str:
    """The engine's framed SHA-256 (`Digest256::framed`) as lowercase hex."""
    data = b"eg/framed-sha256/v1\0" + struct.pack(">I", len(domain)) + domain
    data += struct.pack(">I", len(fields))
    for field in fields:
        data += struct.pack(">Q", len(field)) + field
    return hashlib.sha256(data).hexdigest()


def _resource_grant(
    principal: str, resource: str, granted_at_ms: int, expires_at_ms: int
) -> dict[str, Any]:
    """One knowledge-set grant of `operation: discover`, with its derived id."""
    digest = _framed(
        b"eg/resource-grant/v1",
        [
            b"principal",
            principal.encode(),
            resource.encode(),
            b"knowledge",
            b"discover",
            struct.pack(">Q", granted_at_ms),
        ],
    )
    return {
        "grant_id": f"rgrant:{digest}",
        "subject": {"Principal": principal},
        "resource_ref": resource,
        "protocol": "knowledge",
        "operation": "discover",
        "scope_ceiling": [],
        "granted_at_ms": granted_at_ms,
        "expires_at_ms": expires_at_ms,
    }


def _grant(cl: Any, grant: dict[str, Any]) -> None:
    cl._send(
        "RbacAdmin",
        {"op": {"GrantResource": grant}},
        None,
        idempotency_key=f"grant-{uuid.uuid4().hex}",
    )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R704.4")
def test_resource_grants_default_deny(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R704.4: a knowledge set the agent scope names
    is denied without an unexpired grant naming the caller (none, expired, revoked),
    allowed with one, and a non-admin caller cannot write a grant."""
    graph = f"knowledge-grants-{uuid.uuid4().hex[:8]}"
    resource = f"ks:grants-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    outsider = None
    try:
        _corpus(cl, graph)
        put(cl, graph, resource, {"type": "KnowledgeSet"})
        put(
            cl,
            graph,
            "agent:granted",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"knowledge_sets": [resource]},
            },
        )

        # No grant: denied by default.
        with pytest.raises(Exception, match="RESOURCE_ACCESS_DENIED"):
            _discover(cl, graph, "agent:granted")

        # An expired grant is no grant, and answers like a missing one.
        now_ms = int(time.time() * 1000)
        _grant(
            cl, _resource_grant(TEST_AGENT_ID, resource, now_ms - 2000, now_ms - 1000)
        )
        with pytest.raises(Exception, match="RESOURCE_ACCESS_DENIED"):
            _discover(cl, graph, "agent:granted")

        # An unexpired grant admits the read.
        live = _resource_grant(TEST_AGENT_ID, resource, now_ms, now_ms + 3_600_000)
        _grant(cl, live)
        assert isinstance(_discover(cl, graph, "agent:granted"), list)

        # Revoked, it is denied again.
        cl._send(
            "RbacAdmin",
            {"op": {"RevokeResource": {"grant_id": live["grant_id"]}}},
            None,
            idempotency_key=f"revoke-{uuid.uuid4().hex}",
        )
        with pytest.raises(Exception, match="RESOURCE_ACCESS_DENIED"):
            _discover(cl, graph, "agent:granted")

        # A caller that is not an admin cannot write a grant.
        agent = f"grants-outsider-{uuid.uuid4().hex[:8]}"
        register_commons_agents(agent, graph=graph)
        from medulla.client import SyncMedullaClient

        outsider = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=agent),
        )
        with pytest.raises(Exception, match="security:admin|ACCESS_DENIED|SCOPE"):
            _grant(outsider, _resource_grant(agent, resource, now_ms, now_ms + 60_000))
    finally:
        if outsider is not None:
            outsider.close()
        cl.tenants.delete(graph)
        cl.close()
