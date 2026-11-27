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

    # Only the owner shares: the grantee's attempt is refused, and the
    # stranger's attempt looks like an unknown source.
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
    assert (
        _call(grantee, "ProbeForeignSource", {"name": qualified})["name"] == qualified
    )
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


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R769")
def test_detach_removes_only_the_owners_registration() -> None:
    tag = uuid.uuid4().hex[:8]
    owner_agent, stranger_agent = (
        f"det-{role}-{tag}" for role in ("owner", "stranger")
    )
    owner, stranger = (_client(agent) for agent in (owner_agent, stranger_agent))
    kept, gone = f"detach-kept-{tag}", f"detach-gone-{tag}"
    for name in (kept, gone):
        _call(
            owner, "RegisterForeignSource", {"name": name, "source": SOURCE}, write=True
        )
    # The stranger registers the same name; detaching its own leaves the owner's.
    _call(
        stranger, "RegisterForeignSource", {"name": gone, "source": SOURCE}, write=True
    )

    _call(owner, "ForeignSourceDetach", {"name": gone}, write=True)
    names = [view["name"] for view in _call(owner, "ListForeignSources", {})]
    assert gone not in names and kept in names
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
        _call(owner, "GetForeignSource", {"name": gone})
    assert _call(stranger, "GetForeignSource", {"name": gone})["owned"] is True

    # A second detach, an unregistered name and another owner's name answer alike.
    for name in (gone, f"never-registered-{tag}"):
        _call(owner, "ForeignSourceDetach", {"name": name}, write=True)
    _call(owner, "ForeignSourceDetach", {"name": kept}, write=True)
    _call(stranger, "ForeignSourceDetach", {"name": kept}, write=True)
    _call(stranger, "ForeignSourceDetach", {"name": gone}, write=True)
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND"):
        _call(stranger, "GetForeignSource", {"name": gone})
    for client in (owner, stranger):
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R770")
def test_parity_refusals_are_typed_and_reveal_nothing() -> None:
    tag = uuid.uuid4().hex[:8]
    owner_agent, stranger_agent = (
        f"par-{role}-{tag}" for role in ("owner", "stranger")
    )
    owner, stranger = (_client(agent) for agent in (owner_agent, stranger_agent))
    name = f"parity-{tag}"
    _call(owner, "RegisterForeignSource", {"name": name, "source": SOURCE}, write=True)

    request = {"name": name, "tables": ["accounts"], "sample": 5}
    with pytest.raises(EngineResponseError, match="PARITY_TABLES_INVALID"):
        _call(owner, "ForeignSourceParity", request | {"tables": []})
    with pytest.raises(EngineResponseError, match="PARITY_SAMPLE_INVALID"):
        _call(owner, "ForeignSourceParity", request | {"sample": 0})
    # An unregistered table and another owner's source refuse alike.
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND") as unregistered:
        _call(owner, "ForeignSourceParity", request)
    with pytest.raises(EngineResponseError, match="SOURCE_NOT_FOUND") as foreign:
        _call(stranger, "ForeignSourceParity", request)
    assert str(unregistered.value) == str(foreign.value)
    for client in (owner, stranger):
        client.close()
