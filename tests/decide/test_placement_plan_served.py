"""Served `placement_plan` decision: a capability gap abstains, a feasible plan solves.

spec: MEDULLA-PLACEMENT-R700.6

Hosts are `placement:Host` graph nodes. The first request names a service that needs a
capability no host declares: the decision abstains and names it. After one host declares
the capability the plan solves with the fewest moves.
"""

import uuid

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT

from decide.test_run_work_item_served import authenticated_plan_session
from medulla import decision_client as dc

pytestmark = [pytest.mark.no_engine]

GRAPH = "__commons__"
PLACEMENT = "https://knuckles-team.github.io/ontology/placement#"
GIB = 1 << 30


@pytest.fixture
def authenticated_plan_client(tmp_path):
    yield from authenticated_plan_session(tmp_path)


def _host(host_iri: str, capabilities: list[str]) -> dict:
    return {
        "op": "add_node",
        "id": host_iri,
        "properties": {
            "type": PLACEMENT + "Host",
            PLACEMENT + "cpuMillis": 4_000,
            PLACEMENT + "memoryBytes": 8 * GIB,
            PLACEMENT + "storageBytes": 100 * GIB,
            PLACEMENT + "capabilities": capabilities,
        },
    }


def _service(iri: str, capability: str | None, current: str) -> dict:
    return {
        "service_iri": iri,
        "cpu_millis": 250,
        "memory_bytes": GIB,
        "storage_bytes": GIB,
        "required_capability": capability,
        "current_host": current,
    }


def _decide(client, services: list[dict]) -> dict:
    schema = publish(
        client, "plan-schema-" + uuid.uuid4().hex, "feature_schema", schema_body()
    )
    request = dc.decide_request(
        TEST_TENANT,
        ("deploy.placement_plan", "placement_plan", "ordinary"),
        dc.declared([dc.DeclaredOption("placement_plan:solve")]),
        schema,
        params=[
            dc.param(
                "placement_plan",
                "placement_plan",
                {"graph": GRAPH, "body": {"services": services}},
            )
        ],
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


@pytest.mark.spec("MEDULLA-PLACEMENT-R700.6")
def test_plan_over_wire(authenticated_plan_client):
    client = authenticated_plan_client
    client.lifecycle.batch_update(
        [_host("host:alpha", []), _host("host:beta", []), _host("host:gamma", [])]
    )
    services = [
        _service("svc:a", None, "host:alpha"),
        _service("svc:b", None, "host:beta"),
        _service("svc:c", None, "host:gamma"),
        _service("svc:d", None, "host:alpha"),
        _service("svc:e", "backtest", "host:alpha"),
    ]

    # No host declares `backtest`: the decision abstains and names it.
    gap = _decide(client, services)
    assert gap["outcome"]["outcome"] == "abstained", gap["outcome"]
    assert "uncovered_capability" in str(gap["outcome"]), gap["outcome"]
    assert "backtest" in str(gap["outcome"]), gap["outcome"]

    # One host declares it: the plan solves and only the service that needs it moves.
    client.lifecycle.batch_update([_host("host:delta", ["backtest"])])
    solved = _decide(client, services)
    assert solved["outcome"]["outcome"] == "advisory", solved["outcome"]
    assert [s["option_id"] for s in solved["outcome"]["scores"]] == [
        "placement_plan:solve"
    ]
    plan = solved["answer"]["value"]
    assert len(plan["placements"]) == 5
    assert [m["service_iri"] for m in plan["moves"]] == ["svc:e"]
    assert plan["moves"][0]["to_host"] == "host:delta"
    assert len(plan["certificate_digest"]) == 64
