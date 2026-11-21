"""Two-agent stewardship helpers for the typed-pack served tests.

A change request is opened by one agent and reviewed by another (an approver never
opened the request it approves, `SELF_APPROVAL_DENIED`), so a served test that needs an
approved request speaks as two registered agents.
"""

from __future__ import annotations

import os
from typing import Any

import msgpack
from conftest import register_commons_agents, request_context

from medulla.client import SyncMedullaClient

REVIEWER = "agent-change-reviewer"
STRANGER = "agent-change-stranger"
DIGEST = "sha256:" + "ab" * 32


def connect(graph: str | None = None, *, agent_id: str | None = None) -> Any:
    context = request_context(agent_id=agent_id) if agent_id else request_context()
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=context,
    )


def decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def principal(agent_id: str | None = None) -> str:
    return request_context(agent_id=agent_id)["principal"] if agent_id else request_context()["principal"]


def stewardship(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return decode(client._send("Stewardship", {"op": op}, graph))


def register_agents(graph: str, *agent_ids: str) -> None:
    """Let the named agents read and write `graph`."""
    register_commons_agents(*agent_ids, graph=graph)


def assign_stewards(client: Any, graph: str, asset: str, *who: str) -> None:
    for name in who:
        stewardship(
            client,
            graph,
            {
                "op": "assign",
                "assignment": {
                    "scope": {"kind": "asset", "id": asset},
                    "role": "steward",
                    "principal": name,
                },
            },
        )


def open_request(client: Any, graph: str, asset: str, digest: str = DIGEST) -> str:
    opened = stewardship(
        client,
        graph,
        {
            "op": "open_change_request",
            "target": asset,
            "proposed_digest": digest,
            "scopes": {"asset": asset},
        },
    )
    return opened["id"]


def approved_request(opener: Any, reviewer: Any, graph: str, asset: str) -> str:
    """Steward `asset` by the opener and the reviewer, open a request, have the reviewer approve it."""
    assign_stewards(opener, graph, asset, principal(), principal(REVIEWER))
    request_id = open_request(opener, graph, asset)
    stewardship(reviewer, graph, {"op": "review", "request_id": request_id, "approve": True})
    return request_id
