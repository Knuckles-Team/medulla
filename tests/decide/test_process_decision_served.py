"""MEDULLA-DECISION-ENGINE-R180.7 served proof of process decisions.

A DMN document is imported through ``ProcessDocumentImport``. ``Decide`` with
``decision_table_eval`` then evaluates its UNIQUE table over typed inputs, and
``Decide`` with ``branch_select`` selects the arm of an exclusive branch of a
committed plan whose arm cites that table. ``Decide`` writes nothing.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient
from medulla.decision_client import task_plan_request

GRAPH = "__commons__"

DMN = b"""<?xml version="1.0"?>
<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/"
    id="defs" name="Gold">
  <decision id="gold" name="Gold">
    <decisionTable id="t1" hitPolicy="UNIQUE">
      <input id="i1" label="Tier">
        <inputExpression typeRef="string"><text>tier</text></inputExpression>
      </input>
      <output id="o1" name="ok" typeRef="boolean"/>
      <rule id="rg">
        <inputEntry><text>"gold"</text></inputEntry>
        <outputEntry><text>true</text></outputEntry>
      </rule>
      <rule id="rn">
        <inputEntry><text>not("gold")</text></inputEntry>
        <outputEntry><text>false</text></outputEntry>
      </rule>
    </decisionTable>
  </decision>
</definitions>
"""


def _tier(value: str) -> dict[str, Any]:
    return {"tier": {"type": "string", "value": value}}


def _plan_proposal(decision_iri: str) -> dict[str, Any]:
    def step(task_id: str, deps: list[str], **extra: Any) -> dict[str, Any]:
        return {
            "task_id": task_id,
            "prompt": f"Do {task_id}",
            "depends_on": deps,
            **extra,
        }

    return {
        "tasks": [
            step("start", []),
            step(
                "fork",
                ["start"],
                branch={
                    "kind": "exclusive",
                    "arms": [
                        {
                            "target_task_id": "x",
                            "condition": {"decision_iri": decision_iri},
                        },
                        {"target_task_id": "y"},
                    ],
                    "default_arm": "y",
                },
            ),
            step("x", ["fork"]),
            step("y", ["fork"]),
            step("end", ["x", "y"]),
        ],
        "output_task_id": "end",
        "max_concurrency": 4,
        "max_steps": 8,
    }


def _options(record: dict[str, Any]) -> list[str]:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    return [score["option_id"] for score in outcome["scores"]]


async def _journey(schema: dict[str, Any], model_iri: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    decision_iri = f"{model_iri}#gold"
    importer: Any = client
    try:
        await importer.process_document_import(
            {"format": "dmn13", "document": DMN}, model_iri, 0
        )
        decider = dc.DecisionClient(client=client)

        def ask(kind: str, body: dict[str, Any], name: str) -> dict[str, Any]:
            return dc.decide_request(
                TEST_TENANT,
                (name, kind, "ordinary"),
                dc.declared([dc.DeclaredOption("decision")]),
                schema,
                params=[
                    dc.param("graph", "text", GRAPH),
                    dc.param("process_decision", "text", json.dumps(body)),
                ],
            )

        # The UNIQUE table: gold matches the first rule only.
        table = await decider.decide(
            ask(
                "decision_table_eval",
                {"decision_iri": decision_iri, "inputs": _tier("gold")},
                "pd.table",
            )
        )
        chosen = _options(table["records"][0])
        assert any(option.startswith("rule:") for option in chosen), chosen
        facts = [premise["fact"] for premise in table["records"][0]["premises"]]
        assert "input:tier=gold" in facts, facts

        # The exclusive branch of a committed plan cites the table.
        planned = await decider.decide(
            task_plan_request(
                TEST_TENANT, "pd.plan", _plan_proposal(decision_iri), schema
            )
        )
        plan_record = planned["records"][0]
        plan_digest = plan_record["answer"]["value"]["plan_digest"]
        await decider.log(dc.commit_op(plan_record))
        for tier, taken, default in (("gold", "x", "false"), ("bronze", "y", "true")):
            branch = await decider.decide(
                ask(
                    "branch_select",
                    {
                        "plan_digest": plan_digest,
                        "branch_task_id": "fork",
                        "inputs": _tier(tier),
                    },
                    f"pd.branch.{tier}",
                )
            )
            assert _options(branch["records"][0]) == [
                f"taken:{taken}",
                f"default:{default}",
            ]
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R180.7
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R180.7")
def test_table_and_branch_served(clean_graph):
    schema = publish(
        clean_graph,
        f"r180-7-schema-{uuid.uuid4().hex[:8]}",
        "feature_schema",
        schema_body(),
    )
    asyncio.run(_journey(schema, f"urn:pd:{uuid.uuid4().hex[:8]}"))
