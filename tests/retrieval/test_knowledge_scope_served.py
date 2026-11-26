"""Served agent knowledge scope on Discover (MEDULLA-FEDERATED-QUERY-R704.3).

Two agents read one corpus. Each agent definition node declares its `knowledgeScope`; a
`Discover` that names the agent returns only what its scope admits, and an agent with no
scope sees everything the caller may read.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put


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
        put(
            cl,
            graph,
            "agent:foreign",
            {
                "type": "AgentDefinition",
                "knowledgeScope": {"knowledge_sets": ["ks:theirs"]},
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
