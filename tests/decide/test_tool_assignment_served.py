"""MEDULLA-DECISION-ENGINE-R134.110.6: one ranking for every search surface.

Two principals of one tenant call the served ``Decide`` operation with a
``ToolRelevance`` question over 123 published ``Tool`` components on 66
servers and one ``Toolset`` role. Both callers receive the same order, and it
equals the ranking the engine sealed in the record's premises.
"""

from __future__ import annotations

import os
import re
from typing import Any

import pytest
from _decision_resident_fixture import DIGEST, publication, publish, schema_body
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import SyncMedullaClient

TASK = "export the quarterly sales report to a file"
TOOLS = 123
SERVERS = 66
ROLE = "role-reporting"


def _publish_tool(client: Any, index: int) -> dict[str, Any]:
    component_id = f"tool-{index:03d}"
    if index == 7:
        name, summary = "export report", "export the quarterly sales report to a file"
    elif index == 8:
        name, summary = "export table", "export a table to a file"
    else:
        name, summary = f"tool {index}", f"handles topic{index % 97} requests"
    request = publication(component_id, "tool", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = DIGEST
    component["summary"] = summary
    component["facts"] = {
        "facts": "tool",
        "effect": "read",
        "required_scopes": [],
        "schema_tokens": 100,
    }
    component["attributes"] = {
        "name": name,
        "server_id": f"server-{index % SERVERS:02d}",
    }
    entry = client._send("AgentComponent", request)["component"]
    return {
        "component_id": component_id,
        "kind": "tool",
        "definition_digest": entry["definition_digest"],
    }


def _publish_role(client: Any, tools: list[dict[str, Any]]) -> None:
    request = publication(ROLE, "toolset", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = DIGEST
    component["facts"] = {"facts": "toolset", "transport": "mcp"}
    component["attributes"] = {}
    component["requires"] = tools
    client._send("AgentComponent", request)


def _ask(client: Any, schema: dict[str, Any], role: str) -> dict[str, Any]:
    asked = dc.tool_assignment_request(
        TEST_TENANT,
        "assign.tools",
        schema,
        task_text=TASK,
        agent_role_iri=role,
        top_k=10,
        context_budget_tokens=5000,
    )
    return client._send("Decide", {"request": asked})["records"][0]


def _order(record: dict[str, Any]) -> list[str]:
    return [score["option_id"] for score in record["outcome"]["scores"]]


def _sealed_order(record: dict[str, Any]) -> list[str]:
    ranked: dict[int, str] = {}
    for premise in record["premises"]:
        fact = premise["fact"]
        if not fact.startswith("tool_rank:"):
            continue
        tool = re.search(r"tool=([^;]+);", fact)
        rank = re.search(r"fused_rank=(\d+)", fact)
        assert tool and rank, fact
        ranked[int(rank.group(1))] = tool.group(1)
    return [ranked[position] for position in sorted(ranked)]


# spec: MEDULLA-DECISION-ENGINE-R134.110.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R134.110.6")
def test_role_assignment_uses_one_ranking(clean_graph):
    schema = publish(clean_graph, "tool-schema", "feature_schema", schema_body())
    tools = [_publish_tool(clean_graph, index) for index in range(TOOLS)]
    assert len({f"server-{i % SERVERS:02d}" for i in range(TOOLS)}) == SERVERS
    _publish_role(clean_graph, tools[:40])

    principal_a = clean_graph
    principal_b = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="principal-b"),
    )
    try:
        for role in ("", ROLE):
            first = _ask(principal_a, schema, role)
            second = _ask(principal_b, schema, role)
            order = _order(first)
            assert order, "the catalog answers the task"
            assert order[0] == "tool-007"
            assert _order(second) == order
            assert _sealed_order(first) == order
            assert _sealed_order(second) == order
        scoped = _order(_ask(principal_a, schema, ROLE))
        assert set(scoped) <= {tool["component_id"] for tool in tools[:40]}
    finally:
        principal_b.close()
