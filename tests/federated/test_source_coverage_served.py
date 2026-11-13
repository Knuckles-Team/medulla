"""Served source coverage (MEDULLA-SRC-AUTH-R004.6, R004.6.1).

`SourceCoverage` reports which connected source is authoritative for a class: the
rank-1 source wins, lower ranks are listed as alternates, and every verdict carries
its premises. A source is connected only for the principal that registered it.
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
TEMPLATE = (
    _ROOT / "crates/medulla-rdf/tests/fixtures/source_authority_incident.ttl"
).read_text()
INCIDENT = "https://knuckles-team.github.io/ontology/core#Incident"
GRAPH = "__commons__"
SOURCE = {"Mcp": {"server": "coverage-test-mcp", "tool": "search"}}


def _client(agent: str) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent),
    )


def _agent(tag: str) -> str:
    return f"cov-{tag}-{uuid.uuid4().hex[:8]}"


def _attach(client: Any, servicenow: int, pagerduty: int) -> None:
    document = TEMPLATE.replace("RANK_SERVICENOW", str(servicenow)).replace(
        "RANK_PAGERDUTY", str(pagerduty)
    )
    client._send(
        "GraphSchema",
        {
            "op": {
                "op": "attach",
                "source_id": "authority-incident",
                "ontology_ttl": document,
            }
        },
        GRAPH,
        idempotency_key=f"attach-{uuid.uuid4().hex}",
    )


def _register(client: Any, *names: str) -> None:
    for name in names:
        client._send(
            "RegisterForeignSource",
            {"name": name, "source": SOURCE},
            None,
            idempotency_key=f"register-{name}-{uuid.uuid4().hex}",
        )


def _coverage(client: Any, classes: list[str], **extra: Any) -> dict[str, Any]:
    result = client._send(
        "SourceCoverage", {"classes": classes, "hop_budget": 0, **extra}, GRAPH
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _incident(reply: dict[str, Any]) -> dict[str, Any]:
    return next(v for v in reply["verdicts"] if v["class"] == INCIDENT)


@pytest.mark.spec("MEDULLA-SRC-AUTH-R004.6.1")
def test_rank_one_wins_and_alternates_listed() -> None:
    owner = _client(_agent("a"))
    _register(owner, "servicenow-mcp", "pagerduty-mcp")
    _attach(owner, 1, 2)

    verdict = _incident(_coverage(owner, [INCIDENT]))
    assert verdict["verdict"] == "covered"
    assert verdict["source"] == "servicenow-mcp"
    assert verdict["alternates"] == ["pagerduty-mcp"]
    assert verdict["premises"], "every verdict carries its premises"
    assert any("Authority" in premise for premise in verdict["premises"])
    reply = _coverage(owner, [INCIDENT])
    used = [entry["source"] for entry in reply["audit"]["used"]]
    assert used == ["servicenow-mcp"]

    _attach(owner, 2, 1)
    swapped = _incident(_coverage(owner, [INCIDENT]))
    assert swapped["source"] == "pagerduty-mcp"
    assert swapped["alternates"] == ["servicenow-mcp"]

    refusals = [
        ([], {}, "SOURCE_COVERAGE_NO_CLASSES"),
        ([f"c{n}" for n in range(65)], {}, "SOURCE_COVERAGE_TOO_MANY_CLASSES"),
        ([INCIDENT], {"hop_budget": 9}, "SOURCE_COVERAGE_HOP_BUDGET_INVALID"),
    ]
    for classes, extra, code in refusals:
        with pytest.raises(EngineResponseError) as refused:
            _coverage(owner, classes, **extra)
        assert code in str(refused.value)


@pytest.mark.spec("MEDULLA-SRC-AUTH-R004.6.1")
def test_other_tenants_call_sees_no_verdict_from_these_sources() -> None:
    owner = _client(_agent("a"))
    other = _client(_agent("b"))
    _register(owner, "servicenow-mcp", "pagerduty-mcp")
    _attach(owner, 1, 2)
    assert _incident(_coverage(owner, [INCIDENT]))["verdict"] == "covered"
    verdict = _incident(_coverage(other, [INCIDENT]))
    assert verdict["verdict"] != "covered"


@pytest.mark.spec("MEDULLA-SRC-AUTH-R004.6")
def test_other_principals_source_is_unconnected() -> None:
    owner = _client(_agent("a"))
    other = _client(_agent("b"))
    _register(owner, "servicenow-mcp", "pagerduty-mcp")
    _attach(owner, 1, 2)

    verdict = _incident(_coverage(other, [INCIDENT]))
    assert verdict["verdict"] == "abstained"
    assert verdict["reason"] == "unconnected"
    reply = _coverage(other, [INCIDENT])
    assert not reply["audit"]["used"]
    assert {entry["reason"] for entry in reply["audit"]["not_used"]} == {"unconnected"}
    assert "registered" not in repr(reply)
