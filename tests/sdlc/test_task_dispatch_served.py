"""Run-context assembly for a `TaskPlan` question (MEDULLA-SDLC-R015.2).

L3: a `TaskPlan` question that carries `task_context_row` and `task_context_repo`
gets its run context assembled from the graph behind the ordinary read ACL; the
digest of the context rides in the answer's premises as `task_context.digest`.
"""

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "decide"))

from _decision_resident_fixture import publish, schema_body  # noqa: E402
from conftest import TEST_TENANT  # noqa: E402
from test_task_plan_served import (  # noqa: E402,F401
    _proposal,
    authenticated_plan_client,
)

from medulla.client import EngineResponseError  # noqa: E402
from medulla.decision_client import param, task_plan_request  # noqa: E402

pytestmark = pytest.mark.no_engine  # The test owns its authenticated engine.

GRAPH = "__commons__"


def _node(client, node_id: str, properties: dict) -> None:
    client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
        },
        GRAPH,
    )


def _edge(client, source: str, target: str, relationship: str) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": msgpack.packb(
                {"relationship": relationship}, use_bin_type=True
            ),
        },
        GRAPH,
    )


def _decide(client, pin, repo: str, row: str) -> dict:
    asked = task_plan_request(TEST_TENANT, "context-" + uuid.uuid4().hex[:8], _proposal(), pin)
    asked["params"] = [
        *asked["params"],
        param("task_context_row", "text", row),
        param("task_context_repo", "text", repo),
    ]
    return client._send("Decide", {"request": asked})["records"][0]


def _digest(record: dict) -> str:
    facts = [
        premise["fact"]
        for premise in record["premises"]
        if premise["fact"].startswith("task_context.digest=")
    ]
    assert len(facts) == 1
    return facts[0].split("=", 1)[1]


@pytest.mark.spec("MEDULLA-SDLC-R015.2")
def test_context_holds_row_plan_contracts_and_neighbourhood(
    authenticated_plan_client,
) -> None:
    client = authenticated_plan_client
    try:
        suffix = uuid.uuid4().hex[:8]
        repo, row = f"repo-{suffix}", "DEMO-R001"
        requirement = f"requirement:{repo}:{row}"
        plan = f"plansection:{repo}:specs/a/plan.md#L3"
        contract = f"specsection:{repo}:specs/a/contracts/seam.md#L1"
        symbol = f"symbol:serve_{suffix}"
        _node(client, requirement, {"type": "Requirement", "text": "Serve the row."})
        _node(
            client,
            plan,
            {"type": "PlanSection", "title": "Plan", "start_line": "3", "text": "Plan the row."},
        )
        _node(
            client,
            contract,
            {
                "type": "SpecSection",
                "repo": repo,
                "file": "specs/a/contracts/seam.md",
                "title": "Seam",
                "start_line": "1",
                "text": f"Provider: demo {row}",
            },
        )
        _node(client, symbol, {"type": "CodeSymbolRef", "name": f"serve_{suffix}"})
        _edge(client, plan, requirement, "covers")
        _edge(client, requirement, symbol, "implementedBy")

        pin = publish(client, "task-context-schema-" + suffix, "feature_schema", schema_body())
        first = _digest(_decide(client, pin, repo, row))
        # spec: MEDULLA-SDLC-R015.2
        assert first == _digest(_decide(client, pin, repo, row))

        # A row with no covering plan section refuses.
        _node(client, f"requirement:{repo}:DEMO-R002", {"type": "Requirement", "text": "x"})
        with pytest.raises(EngineResponseError, match="TASK_CONTEXT_PLAN_MISSING"):
            _decide(client, pin, repo, "DEMO-R002")
        with pytest.raises(EngineResponseError, match="TASK_CONTEXT_ROW_UNKNOWN"):
            _decide(client, pin, repo, "DEMO-R404")
    finally:
        client.close()
