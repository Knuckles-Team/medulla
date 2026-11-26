"""MEDULLA-DECISION-ENGINE-R156.6 served skill-to-workflow compilation.

A typed draft extracted from a skill document is compiled through ``Decide``
(``skill_workflow_compile``), the sealed record is committed through
``DecisionLog``, and ``ProgramLookup`` returns the stored program by the task
signature ``skill:<document digest>``. An incomplete draft refuses by code and
step. No workflow file exists at any point.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
from typing import Any

import pytest
from _decision_resident_fixture import publish
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError, MedullaClient

DOCUMENT = hashlib.sha256(b"deploy skill r156.6").hexdigest()
SIGNATURE = f"skill:{DOCUMENT}"


def _step(
    step_id: str,
    action: str,
    next_steps: list[str],
    on_error: str | None,
) -> dict[str, Any]:
    return {
        "step_id": step_id,
        "action": action,
        "input_types": ["text"],
        "output_types": ["json"],
        "next": [{"step_id": target, "label": None} for target in next_steps],
        "on_error": on_error,
    }


def _draft(steps: list[dict[str, Any]]) -> dict[str, Any]:
    return {"skill_document_digest": DOCUMENT, "steps": steps}


GOLDEN = _draft(
    [
        _step("fetch", "tool.fetch", ["parse"], "recover"),
        _step("parse", "tool.parse", ["store"], "recover"),
        _step("store", "tool.store", [], "recover"),
        _step("recover", "internal.recover", [], None),
    ]
)


def _compile_request(schema: dict[str, Any], draft: dict[str, Any]) -> dict[str, Any]:
    return dc.decide_request(
        TEST_TENANT,
        ("q:skill:compile", "skill_workflow_compile", "ordinary"),
        dc.declared([dc.DeclaredOption("workflow")]),
        schema,
        params=[dc.param("skill_workflow", "skill_workflow_draft", draft)],
    )


def _lookup_request(schema: dict[str, Any]) -> dict[str, Any]:
    return dc.decide_request(
        TEST_TENANT,
        (f"q:program:{SIGNATURE}", "program_lookup", "ordinary"),
        dc.declared([dc.DeclaredOption("program")]),
        schema,
        params=[dc.param("task_signature", "text", SIGNATURE)],
    )


def _program_from(payload: dict[str, Any]) -> dict[str, Any]:
    outcome = payload["records"][0]["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    option_id = outcome["scores"][0]["option_id"]
    assert option_id.startswith("program:")
    return json.loads(option_id.removeprefix("program:"))


async def _journey(schema: dict[str, Any]) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        before = await decision.decide(_lookup_request(schema))
        compiled = await decision.decide(_compile_request(schema, GOLDEN))
        record = compiled["records"][0]
        committed = await decision.log(dc.commit_op(record))
        found = await decision.decide(_lookup_request(schema))
        incomplete = _draft(
            [
                _step("fetch", "tool.fetch", [], None),
                _step("done", "internal.done", [], None),
            ]
        )
        try:
            await decision.decide(_compile_request(schema, incomplete))
            refused = None
        except EngineResponseError as error:
            refused = str(error)
        return {
            "before": before,
            "record": record,
            "committed": committed,
            "found": found,
            "refused": refused,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R156.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R156.6")
def test_draft_compiles_commits_and_looks_up(clean_graph):
    schema = publish(
        clean_graph,
        "r156-6-schema",
        "feature_schema",
        ds.feature_schema_body([ds.feature("n", ds.number("n"))]),
    )
    result = asyncio.run(_journey(schema))

    # Nothing is stored before the commit: the lookup abstains.
    assert result["before"]["records"][0]["outcome"]["outcome"] == "abstained"

    # The sealed record carries the compiled program, a claim under a constraint.
    record = result["record"]
    program = record["answer"]["value"]
    assert record["answer"]["type"] == "skill_workflow"
    assert program["task_signature"] == SIGNATURE
    assert [step["state"] for step in program["steps"]] == [
        "fetch",
        "parse",
        "store",
        "recover",
    ]
    assert record["resolution_kind"] == "constraint"
    assert record["evidence_class"] == "claim"
    facts = {premise["fact"] for premise in record["premises"]}
    assert f"skill_workflow:document={DOCUMENT}" in facts
    assert f"skill_workflow:program={program['program_digest']}" in facts

    # The commit stored it: the lookup by the skill digest returns the same program.
    assert result["committed"]["replayed"] is False
    assert _program_from(result["found"]) == program

    # An incomplete draft refuses by code and step, and stored nothing new.
    assert result["refused"] is not None
    assert "SKILL_WORKFLOW_MISSING_ERROR_HANDLER" in result["refused"]
    assert "step fetch" in result["refused"]
