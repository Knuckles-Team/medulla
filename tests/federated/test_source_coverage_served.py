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
from conftest import register_commons_agents, request_context

from medulla.client import EngineResponseError, SyncMedullaClient

_ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = (
    _ROOT / "crates/medulla-rdf/tests/fixtures/source_authority_incident.ttl"
).read_text()
TWO_SOURCES = (
    _ROOT / "tests/fixtures/source_authority/incident_two_sources.ttl"
).read_text()
INCIDENT = "https://knuckles-team.github.io/ontology/core#Incident"
GRAPH = "__commons__"
SOURCE = {"Mcp": {"server": "coverage-test-mcp", "tool": "search"}}


@pytest.fixture(autouse=True)
def _detach_after() -> Any:
    """Detach this module's schema source after each test.

    The source's authority rows live in the shared commons graph; left attached,
    they would rank for the same class in every later coverage test of the run.
    """
    yield
    client = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        client._send(
            "GraphSchema",
            {"op": {"op": "detach", "source_id": "admin:authority-incident"}},
            GRAPH,
            idempotency_key=f"detach-{uuid.uuid4().hex}",
        )
    except EngineResponseError:
        pass  # this test never attached the source
    finally:
        client.close()


def _client(agent: str | None = None) -> Any:
    """A client for `agent`, registered first; with no agent, the System identity.

    Attaching a schema source is an admin action, so the System identity attaches;
    every other call speaks as the registered principal under test.
    """
    if agent is None:
        return SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
    register_commons_agents(agent)
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id=agent),
    )


def _agent(tag: str) -> str:
    return f"cov-{tag}-{uuid.uuid4().hex[:8]}"


def _attach(servicenow: int, pagerduty: int) -> None:
    _attach_document(
        TEMPLATE.replace("RANK_SERVICENOW", str(servicenow)).replace(
            "RANK_PAGERDUTY", str(pagerduty)
        )
    )


def _attach_document(document: str) -> None:
    client = _client()
    try:
        client._send(
            "GraphSchema",
            {
                "op": {
                    "op": "attach",
                    "source_id": "admin:authority-incident",
                    "ontology_ttl": document,
                }
            },
            GRAPH,
            idempotency_key=f"attach-{uuid.uuid4().hex}",
        )
    finally:
        client.close()


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
    _attach(1, 2)

    verdict = _incident(_coverage(owner, [INCIDENT]))
    assert verdict["verdict"] == "covered", verdict.get("premises")
    assert verdict["source"] == "servicenow-mcp"
    assert verdict["alternates"] == ["pagerduty-mcp"]
    assert verdict["premises"], "every verdict carries its premises"
    assert any("Authority" in premise for premise in verdict["premises"])
    reply = _coverage(owner, [INCIDENT])
    used = [entry["source"] for entry in reply["audit"]["used"]]
    assert used == ["servicenow-mcp"]

    _attach(2, 1)
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
    _attach(1, 2)
    mine = _incident(_coverage(owner, [INCIDENT]))
    assert mine["verdict"] == "covered", mine.get("premises")
    verdict = _incident(_coverage(other, [INCIDENT]))
    assert verdict["verdict"] != "covered"


@pytest.mark.spec("MEDULLA-SRC-AUTH-R004.6")
def test_other_principals_source_is_unconnected() -> None:
    owner = _client(_agent("a"))
    other = _client(_agent("b"))
    _register(owner, "servicenow-mcp", "pagerduty-mcp")
    _attach(1, 2)

    verdict = _incident(_coverage(other, [INCIDENT]))
    assert verdict["verdict"] == "abstained", verdict.get("premises")
    assert verdict["reason"] == "unconnected", verdict.get("premises")
    reply = _coverage(other, [INCIDENT])
    assert not reply["audit"]["used"]
    assert {entry["reason"] for entry in reply["audit"]["not_used"]} == {"unconnected"}
    assert "registered" not in repr(reply)


@pytest.mark.spec("MEDULLA-SRC-AUTH-R009.4")
def test_swapping_authority_rank_changes_sources_used() -> None:
    owner = _client(_agent("swap"))
    _register(owner, "servicenow-mcp", "pagerduty-mcp")
    request = {"classes": [INCIDENT], "hop_budget": 0}
    request_bytes = msgpack.packb(request)

    # ServiceNow rank 1, PagerDuty rank 2: the audit names ServiceNow as the source used.
    _attach_document(TWO_SOURCES)
    first = _coverage(owner, **request)
    assert [entry["source"] for entry in first["audit"]["used"]] == ["servicenow-mcp"]
    assert _incident(first)["source"] == "servicenow-mcp"

    # Swap the two ranks of the same two vg:SourceAuthority rows and ask again.
    swapped = TWO_SOURCES.replace("vg:authorityRank 1 ;", "vg:authorityRank TMP ;")
    swapped = swapped.replace("vg:authorityRank 2 ;", "vg:authorityRank 1 ;")
    swapped = swapped.replace("vg:authorityRank TMP ;", "vg:authorityRank 2 ;")
    assert swapped != TWO_SOURCES
    _attach_document(swapped)
    second = _coverage(owner, **request)

    # The winner changes; the request bytes do not.
    assert msgpack.packb(request) == request_bytes
    assert [entry["source"] for entry in second["audit"]["used"]] == ["pagerduty-mcp"]
    assert _incident(second)["source"] == "pagerduty-mcp"
    assert first["audit"]["used"] != second["audit"]["used"]
