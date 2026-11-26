"""Served agent knowledge scope on Discover (MEDULLA-FEDERATED-QUERY-R704.3).

Two agents read one corpus. Each agent definition node declares its `knowledgeScope`; a
`Discover` that names the agent returns only what its scope admits, and an agent with no
scope sees everything the caller may read.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put


def _discover(cl: Any, graph: str, agent_ref: str | None) -> list[str]:
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
    return sorted(hit["id"] for hit in result["hits"])


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
