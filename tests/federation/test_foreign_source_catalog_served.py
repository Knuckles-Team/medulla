"""Served foreign source catalog (MEDULLA-UNIFIED-DATA-PLANE-R765).

The owner lists, reads and probes its own source. A grantee sees it by its qualified
name once the owner shares it; a stranger never does. Only the owner shares or unshares,
and a revoked grantee's next probe is refused like a name nobody registered.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import register_commons_agents, request_context

from medulla.client import EngineResponseError, SyncMedullaClient

SOURCE = {"Mcp": {"server": "catalog-test-mcp", "tool": "search"}}


def _client(agent: str) -> Any:
    register_commons_agents(agent)
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent),
    )


def _call(client: Any, method: str, params: dict[str, Any], *, write: bool = False):
    key = f"{method}-{uuid.uuid4().hex}" if write else None
    result = client._send(method, params, None, idempotency_key=key)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R765")
def test_visibility_probe_and_revocation() -> None:
    tag = uuid.uuid4().hex[:8]
    owner_agent, grantee_agent, stranger_agent = (
        f"cat-{role}-{tag}" for role in ("owner", "grantee", "stranger")
    )
    owner, grantee, stranger = (
        _client(agent) for agent in (owner_agent, grantee_agent, stranger_agent)
    )
    name = f"catalog-{tag}"
    qualified = f"{owner_agent}/{name}"
    _call(
        owner,
        "RegisterForeignSource",
        {"name": name, "source": SOURCE},
        write=True,
    )

    # The owner sees its source by the plain name, with no configuration in the view.
    own = _call(owner, "GetForeignSource", {"name": name})
    assert (own["name"], own["owner"], own["owned"]) == (name, owner_agent, True)
    assert own["kind"] == "Mcp"
    assert "catalog-test-mcp" not in repr(own)
    assert name in [view["name"] for view in _call(owner, "ListForeignSources", {})]
    assert _call(owner, "ProbeForeignSource", {"name": name})["name"] == name

    # Nobody else sees it before the share.
    for other in (grantee, stranger):
        assert qualified not in [
            view["name"] for view in _call(other, "ListForeignSources", {})
        ]
        with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
            _call(other, "GetForeignSource", {"name": qualified})

    # Only the owner shares: the grantee's attempt is refused, the stranger's looks unknown.
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
        _call(
            stranger,
            "ShareForeignSource",
            {"name": qualified, "grantees": [stranger_agent]},
            write=True,
        )
    _call(
        owner,
        "ShareForeignSource",
        {"name": name, "grantees": [grantee_agent]},
        write=True,
    )
    shared = _call(grantee, "GetForeignSource", {"name": qualified})
    assert (shared["name"], shared["owned"]) == (qualified, False)
    assert _call(grantee, "ProbeForeignSource", {"name": qualified})["name"] == qualified
    with pytest.raises(EngineResponseError, match="SOURCE_OWNER_REQUIRED"):
        _call(
            grantee,
            "ShareForeignSource",
            {"name": qualified, "grantees": [stranger_agent]},
            write=True,
        )
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
        _call(stranger, "GetForeignSource", {"name": qualified})

    # Revocation blocks the next probe.
    _call(
        owner,
        "UnshareForeignSource",
        {"name": name, "grantees": [grantee_agent]},
        write=True,
    )
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
        _call(grantee, "ProbeForeignSource", {"name": qualified})
    for client in (owner, grantee, stranger):
        client.close()
