"""Served source roles (MEDULLA-SRC-AUTH-R010.3).

`SourceCoverage` passes each source's declared `vg:sourceRole` into selection, so
at equal rank the `live_truth` source wins over a `historical_context` one.
"""

from __future__ import annotations

import os
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient

_ROOT = Path(__file__).resolve().parents[2]
CONFLICT = (_ROOT / "tests/fixtures/source_authority/role_conflict.ttl").read_text()
INCIDENT = "https://knuckles-team.github.io/ontology/core#Incident"
GRAPH = "__commons__"
SOURCE = {"Mcp": {"server": "role-test-mcp", "tool": "search"}}


def _client() -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=f"role-{uuid.uuid4().hex[:8]}"),
    )


def _attach(client: Any, document: str) -> None:
    client._send(
        "GraphSchema",
        {
            "op": {
                "op": "attach",
                "source_id": "authority-roles",
                "ontology_ttl": document,
            }
        },
        GRAPH,
        idempotency_key=f"attach-{uuid.uuid4().hex}",
    )


def _coverage(client: Any) -> dict[str, Any]:
    result = client._send(
        "SourceCoverage", {"classes": [INCIDENT], "hop_budget": 0}, GRAPH
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


@pytest.mark.spec("MEDULLA-SRC-AUTH-R010.3")
def test_live_truth_wins_conflict() -> None:
    owner = _client()
    for name in ("live-incidents-mcp", "history-incidents-mcp"):
        owner._send(
            "RegisterForeignSource",
            {"name": name, "source": SOURCE},
            None,
            idempotency_key=f"register-{name}-{uuid.uuid4().hex}",
        )
    _attach(owner, CONFLICT)
    verdict = next(v for v in _coverage(owner)["verdicts"] if v["class"] == INCIDENT)
    assert verdict["verdict"] == "covered"
    assert verdict["source"] == "live-incidents-mcp"
    assert "history-incidents-mcp" in verdict["alternates"]
    assert any("Role" in premise for premise in verdict["premises"])


@pytest.mark.spec("MEDULLA-SRC-AUTH-R010.3")
def test_unknown_source_role_is_refused_at_attach() -> None:
    owner = _client()
    with pytest.raises(EngineResponseError) as refused:
        _attach(owner, CONFLICT.replace('"live_truth"', '"primary"'))
    assert "SOURCE_ROLE_UNKNOWN" in str(refused.value)
