"""MEDULLA-CQ-ORCH-R704.8 served process plan over the wire client.

The entry point is the served ``Decide`` method with the ``process_plan`` question kind.
A canonical workflow with a tool step, two prompt steps, a business rule step, a user step and
an exclusive gateway is stored through ``process_model_import``; the binding links of its
step nodes are written through ``batch_update``. The plan answers one tool step, prompt
steps, one decision step, one approval wait and one branch step with two arms, with one
``binding:`` premise per step. A workflow nobody stored is the family's abstention.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import DIGEST, publication, publish, schema_body
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

SCHEMA = "medulla.process.canonical/v1"

STEPS = [
    {"kind": "start", "label": "Invoice received"},
    {"kind": "agent", "label": "Post the invoice", "role": "poster"},
    {"kind": "agent", "label": "Draft the note", "role": "writer"},
    {"kind": "decision", "label": "Route by amount", "ref": "dmn:route"},
    {"kind": "human", "label": "Clerk review", "role": "clerk"},
    {"kind": "branch", "gate": "xor", "label": "Large amount?"},
    {"kind": "agent", "label": "Archive", "role": "writer"},
    {"kind": "agent", "label": "Escalate", "role": "writer"},
    {"kind": "join", "gate": "xor", "label": "Merged"},
    {"kind": "end", "label": "Done"},
]
FLOWS = [
    ("s1", "s2", None, False),
    ("s2", "s3", None, False),
    ("s3", "s4", None, False),
    ("s4", "s5", None, False),
    ("s5", "s6", None, False),
    ("s6", "s7", "amount > 1000", False),
    ("s6", "s8", None, True),
    ("s7", "s9", None, False),
    ("s8", "s9", None, False),
    ("s9", "s10", None, False),
]


def _canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _workflow(
    step_list: list[dict[str, Any]] = STEPS,
    flow_list: list[tuple[str, str, str | None, bool]] = FLOWS,
) -> dict[str, Any]:
    steps = [{"id": f"s{n}", **step} for n, step in enumerate(step_list, start=1)]
    flows = []
    for source, target, condition, default in flow_list:
        flow: dict[str, Any] = {"source": source, "target": target, "default": default}
        if condition:
            flow["condition"] = condition
        flows.append(flow)
    digest = hashlib.sha256(
        _canonical_json({"schema": SCHEMA, "steps": steps, "flows": flows}).encode()
    ).hexdigest()
    return {
        "schema": SCHEMA,
        "schema_version": 1,
        "name": "Invoice approval",
        "steps": steps,
        "flows": flows,
        "digest": digest,
        "source": {
            "schema_version": 1,
            "system": "bpmn20",
            "model_id": "invoice-approval",
            "elements": {step["id"]: step["id"] for step in steps},
        },
    }


def _node(node_id: str, properties: dict[str, Any]) -> dict[str, Any]:
    return {"op": "upsert_node", "id": node_id, "properties": properties}


def _edge(source: str, target: str, relationship: str) -> dict[str, Any]:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


def _bindings(node_id: str) -> list[dict[str, Any]]:
    """The tool, the prompt, the agent kind and the links of the step nodes."""
    step = lambda n: f"{node_id}:step:s{n}"  # noqa: E731
    operations = [
        _node("tool:post", {"name": "post_invoice", "ofConnector": "ledger-mcp"}),
        _node("prompt:note", {"name": "note"}),
        _node("kind:writer", {"name": "writer-agent"}),
        _node("lane:writer", {"name": "writer"}),
        _edge("lane:writer", "kind:writer", "hasAgentKind"),
        _edge(step(2), "tool:post", "usesTool"),
    ]
    for n in (3, 7, 8):
        operations.append(_edge(step(n), "prompt:note", "hasPrompt"))
    return operations


async def _connect(graph: str) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )


async def _plan(schema: dict[str, Any], graph: str, workflow_id: str, version: int):
    client = await _connect(graph)
    try:
        request = dc.process_plan_request(
            TEST_TENANT,
            f"q:r704-8:{workflow_id}:{version}",
            schema,
            graph=graph,
            workflow_id=workflow_id,
            version=version,
        )
        payload = await dc.DecisionClient(client=client).decide(request)
        return payload["records"][0]
    finally:
        await client.close()


async def _store(graph: str, workflow_id: str, workflow: dict[str, Any] | None = None) -> int:
    client = await _connect(graph)
    try:
        if workflow is not None:
            imported = await client.process_model_import(workflow, workflow_id)
            return imported["version"]
        imported = await client.process_model_import(_workflow(), workflow_id)
        await client.batch_update(_bindings(imported["workflow_id"]))
        return imported["version"]
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R704.8")
def test_process_plan_binds_every_step_kind(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R704.8
    suffix = uuid.uuid4().hex[:8]
    graph = _graph_name(clean_graph)
    schema = publish(clean_graph, f"r704-8-schema-{suffix}", "feature_schema", schema_body())
    workflow_id = f"workflow:invoice-{suffix}"
    version = asyncio.run(_store(graph, workflow_id))

    record = asyncio.run(_plan(schema, graph, workflow_id, version))
    assert record["question"]["kind"] == "process_plan"
    tasks = {task["task_id"]: task for task in record["answer"]["value"]["tasks"]}
    assert set(tasks) == {f"s{n}" for n in range(2, 10)}, "start and end add no task"
    # One tool step, prompt steps, one decision step, one approval wait, one branch step
    # with two arms and the join that closes it.
    assert tasks["s2"]["call"]["tool"] == "post_invoice"
    assert all("call" not in tasks[f"s{n}"] for n in (3, 7, 8))
    assert tasks["s4"]["decision"]["decision_iri"] == "dmn:route"
    assert tasks["s5"]["approval"]["approver_role"] == "clerk"
    branch = tasks["s6"]["branch"]
    assert branch["kind"] == "exclusive"
    assert [arm["target_task_id"] for arm in branch["arms"]] == ["s7"]
    assert branch["default_arm"] == "s8"
    assert tasks["s9"]["join"]["branch_task_id"] == "s6"
    facts = [premise["fact"] for premise in record["premises"]]
    assert any(f.startswith("binding:step=s2;target=tool:ledger-mcp/post_invoice") for f in facts)
    assert sum(f.startswith("binding:step=") for f in facts) == 6

    # Evaluate-only: the same question seals the same plan.
    again = asyncio.run(_plan(schema, graph, workflow_id, version))
    assert again["answer"]["value"]["plan_digest"] == record["answer"]["value"]["plan_digest"]

    # A workflow nobody stored is the family's abstention.
    missing = asyncio.run(_plan(schema, graph, f"workflow:none-{suffix}", 1))
    assert missing["outcome"]["outcome"] == "abstained", missing["outcome"]
    assert "WORKFLOW_UNKNOWN" in json.dumps(missing["outcome"])


RANKED_TASK = "export the quarterly sales report to a file"


def _publish_ranked_tool(client: Any, component_id: str, server: str, summary: str) -> None:
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
    component["attributes"] = {"name": component_id, "server_id": server}
    client._send("AgentComponent", request)


@pytest.mark.spec("MEDULLA-CQ-ORCH-R704.8")
def test_process_plan_ranked_binding_is_a_claim(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R704.8
    """An agent step the graph does not bind takes the top ranked tool that has a registered
    contract; the binding premise is a ranked claim, and its plan carries the tool call."""
    suffix = uuid.uuid4().hex[:8]
    graph = _graph_name(clean_graph)
    schema = publish(clean_graph, f"r704-8-ranked-schema-{suffix}", "feature_schema", schema_body())
    tool = f"export-report-{suffix}"
    server = f"reports-{suffix}"
    _publish_ranked_tool(clean_graph, tool, server, RANKED_TASK)
    for index in range(5):
        _publish_ranked_tool(
            clean_graph,
            f"filler-{suffix}-{index}",
            f"filler-server-{index}",
            f"handles topic{index} requests",
        )
    workflow = _workflow(
        [
            {"kind": "start", "label": "Report requested"},
            {"kind": "agent", "label": RANKED_TASK},
            {"kind": "end", "label": "Done"},
        ],
        [("s1", "s2", None, False), ("s2", "s3", None, False)],
    )
    workflow_id = f"workflow:ranked-{suffix}"
    version = asyncio.run(_store(graph, workflow_id, workflow))

    async def register_contract() -> None:
        client = await _connect(graph)
        try:
            await client.batch_update(
                [
                    _node(
                        f"tool-node:{suffix}",
                        {
                            "name": tool,
                            "ofConnector": server,
                            "contractIri": f"urn:contract:{suffix}",
                        },
                    )
                ]
            )
        finally:
            await client.close()

    asyncio.run(register_contract())
    record = asyncio.run(_plan(schema, graph, workflow_id, version))
    tasks = {task["task_id"]: task for task in record["answer"]["value"]["tasks"]}
    assert tasks["s2"]["call"]["tool"] == tool
    ranked = [
        premise
        for premise in record["premises"]
        if premise["fact"].startswith("binding:step=s2;")
    ]
    assert len(ranked) == 1, record["premises"]
    assert f"target=tool:{server}/{tool}" in ranked[0]["fact"]
    assert "source=ranked" in ranked[0]["fact"]
    assert ranked[0]["class"] == "claim", ranked[0]
