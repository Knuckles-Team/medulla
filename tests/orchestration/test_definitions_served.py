"""MEDULLA-ORCH-VOCAB-R800 served definitions.

L4: every call goes through the running engine over the wire client. A prompt template
and a workflow pattern are graph nodes of the request graph; ``AgentAssemble`` reads the
ones the request names and renders each assigned prompt. An agent definition is an Agent
Library entry: an edit publishes a new revision and the champion pointer selects the
current one.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publication
from composed_support import publish_composed_library
from conftest import TEST_TENANT
from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)

BODY = "Hi {{name}}: {{count}} items for {{task}} ({{tags}})"
SLOTS = [
    {"name": "name", "slot_type": "text"},
    {"name": "count", "slot_type": "integer"},
    {"name": "task", "slot_type": "iri"},
    {"name": "tags", "slot_type": "list"},
]
VALUES = {
    "name": {"type": "text", "value": "Ada"},
    "count": {"type": "integer", "value": 3},
    "task": {"type": "iri", "value": "urn:task:xyz"},
    "tags": {"type": "list", "value": ["a", "b"]},
}


def _put_template(client: Any, template_id: str, body: str = BODY) -> str:
    """Store one template node; returns its digest."""
    digest = hashlib.sha256(body.encode()).hexdigest()
    client.nodes.add(
        f"prompttemplate:{template_id}",
        {
            "node_type": "PromptTemplate",
            "tenant_id": TEST_TENANT,
            "revision_ref": f"program:{template_id}",
            "slots": json.dumps(SLOTS),
            "digest": f"sha256:{digest}",
            "body": body,
        },
    )
    return f"sha256:{digest}"


def _assemble(client: Any, **fields: Any) -> dict[str, Any]:
    request = {
        "tenant_id": TEST_TENANT,
        "policy": {"policy": "default"},
        "candidates": {"kinds": ["tool"]},
        "requirements": {},
    }
    request.update(fields)
    return client._send("AgentAssemble", {"request": request})


def _prompt(template_id: str, values: dict[str, Any]) -> dict[str, Any]:
    return {
        "agent_slot": 0,
        "template_id": template_id,
        "values": [{"name": name, "value": value} for name, value in values.items()],
    }


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R800.6")
def test_assembly_renders_prompts_for_each_assigned_agent(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R800.6
    suffix = f"-{uuid.uuid4().hex[:8]}"
    publish_composed_library(clean_graph, suffix)
    template_id = f"greet{suffix}"
    digest = _put_template(clean_graph, template_id)

    result = _assemble(clean_graph, prompts=[_prompt(template_id, VALUES)])
    assert result["agents"], "the assembly staffs at least one agent"
    rendered = result["rendered_prompts"]
    assert len(rendered) == 1
    assert rendered[0]["agent_slot"] == 0
    assert rendered[0]["template_id"] == template_id
    assert rendered[0]["template_digest"] in (digest, digest.removeprefix("sha256:"))
    assert rendered[0]["text"] == "Hi Ada: 3 items for urn:task:xyz (a, b)"
    assert len(rendered[0]["render_digest"]) == 64
    # The same template and values render to the same digest.
    again = _assemble(clean_graph, prompts=[_prompt(template_id, VALUES)])
    assert again["rendered_prompts"][0]["render_digest"] == rendered[0]["render_digest"]
    # A value that holds a placeholder is kept verbatim: values are never re-scanned.
    tricky = dict(VALUES, name={"type": "text", "value": "{{count}}"})
    kept = _assemble(clean_graph, prompts=[_prompt(template_id, tricky)])
    assert kept["rendered_prompts"][0]["text"].startswith("Hi {{count}}: 3 items")


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R800.4")
def test_four_faults_refuse_with_codes(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R800.4
    suffix = f"-{uuid.uuid4().hex[:8]}"
    publish_composed_library(clean_graph, suffix)
    template_id = f"faults{suffix}"
    _put_template(clean_graph, template_id)

    unfilled = {name: value for name, value in VALUES.items() if name != "tags"}
    with pytest.raises(Exception, match="TEMPLATE_SLOT_UNFILLED"):
        _assemble(clean_graph, prompts=[_prompt(template_id, unfilled)])

    mistyped = dict(VALUES, count={"type": "text", "value": "three"})
    with pytest.raises(Exception, match="TEMPLATE_SLOT_TYPE"):
        _assemble(clean_graph, prompts=[_prompt(template_id, mistyped)])

    pattern_id = f"workflow:process:faults{suffix}:v1"
    clean_graph.nodes.add(
        pattern_id,
        {
            "node_type": "WorkflowPattern",
            "tenant_id": TEST_TENANT,
            "process_iri": f"process:faults{suffix}",
            "version": 1,
            "steps": json.dumps(["s1", "s9"]),
            "workflowDigest": "ab" * 32,
            "canonical": json.dumps({"steps": [{"id": "s1"}, {"id": "s2"}]}),
            "preconditions": "[]",
        },
    )
    with pytest.raises(Exception, match="PATTERN_STEP_UNKNOWN"):
        _assemble(clean_graph, patterns=[pattern_id])

    # A refusal writes nothing: a clean request still assembles.
    assert _assemble(clean_graph)["agents"]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R800.3")
def test_edit_creates_new_revision_and_champion_selects_current(
    authenticated_plan_client,  # noqa: F811
):
    # spec: MEDULLA-ORCH-VOCAB-R800.3
    from medulla import decision_client as dc

    client = authenticated_plan_client
    agent_id = f"agent:definition-{uuid.uuid4().hex[:8]}"
    subject = {
        "kind": "definition",
        "definition_kind": "agent",
        "definition_ref": agent_id,
    }
    try:
        first = _publish_revision_sync(client, agent_id, 0, "v1")
        second = _publish_revision_sync(client, agent_id, 1, "v2")
        # An edit is a new revision: the first stays exactly as published.
        assert first["entry_revision"] == 1 and second["entry_revision"] == 2
        history = _history(client, agent_id)
        assert [entry["entry_revision"] for entry in history] == [1, 2]
        assert history[0]["definition_digest"] == first["definition_digest"]

        # An edit that reuses the published revision is refused and changes nothing.
        with pytest.raises(
            Exception,
            match="DEFINITION_REVISION_PUBLISHED|STALE_AGENT_LIBRARY_REVISION",
        ):
            _publish_revision_sync(client, agent_id, 0, "v1-again")
        again = _history(client, agent_id)
        assert again == history

        # The current revision is the champion pointer, not the newest write.
        for target in (first, second):
            record_id = _promote(client, subject, target["definition_digest"])
            moved = client._send(
                "DecisionLog",
                {
                    "op": dc.learn_op(
                        TEST_TENANT,
                        "move_pointer",
                        pointer={"pointer": "champion", "subject": subject},
                        movement={
                            "movement": "activate",
                            "target": target["definition_digest"],
                            "receipt_digest": record_id,
                        },
                    )
                },
            )
            assert moved["recorded"] == "pointer", moved
            pointer = client._send(
                "DecisionLog",
                {
                    "op": dc.pointer_op(
                        TEST_TENANT, f"champion:definition:agent:{agent_id}"
                    )
                },
            )
            assert pointer["revision_ref"] == target["definition_digest"], pointer
    finally:
        client.close()


def _history(client: Any, agent_id: str) -> list[dict[str, Any]]:
    return client._send(
        "AgentComponent",
        {
            "op": {
                "op": "history",
                "tenant_id": TEST_TENANT,
                "component_id": agent_id,
            }
        },
    )


def _publish_revision_sync(
    client: Any, agent_id: str, expected: int, content: str
) -> dict[str, Any]:
    request = publication(agent_id, "tool", {})
    inner = request["op"]["request"]
    inner["context"]["expected_revision"] = expected
    inner["context"]["idempotency_key"] = f"{agent_id}:{expected}:{content}"
    inner["component"]["content_digest"] = (
        "sha256:" + hashlib.sha256(content.encode()).hexdigest()
    )
    return client._send("AgentComponent", request)["component"]


def _promote(client: Any, subject: dict[str, Any], target: str) -> str:
    """Promote ``target`` as the champion of ``subject``; the committed record id."""
    from _decision_resident_fixture import publish, schema_body

    from medulla import decision_client as dc

    schema = publish(
        client,
        f"definition-schema-{uuid.uuid4().hex[:8]}",
        "feature_schema",
        schema_body(),
    )
    evaluation: dict[str, Any] = {
        "subject": subject,
        "revision_ref": target,
        "champion_revision_ref": None,
        "evaluation_set_digest": "sha256:" + "10" * 32,
        "evaluator_digest": "sha256:" + "11" * 32,
        "evaluator_kind": "annotation",
        "held_out_trajectory_ids": [],
        "metrics": {
            "task_success_ppm": 900_000,
            "baseline_task_success_ppm": 850_000,
            "cost_micros": 12,
            "latency_p50_ms": 4,
            "latency_p95_ms": 9,
            "gpu_seconds": 2,
            "trace_completeness_ppm": 1_000_000,
            "unsupported_replay_mass_ppm": 0,
        },
        "safety": "passed",
        "verdict": "accepted",
        "report": None,
    }
    receipt = client._send(
        "PolicyEvolution",
        {"op": {"op": "commit_policy_evaluation", "request": evaluation}},
    )
    asked = dc.decide_request(
        TEST_TENANT,
        ("promote.definition", "promotion", "ordinary"),
        dc.declared([dc.DeclaredOption(target)]),
        schema,
        params=[
            dc.param("mode", "text", "promote"),
            dc.param("receipt_digest", "text", receipt["record_id"]),
            dc.param("subject", "text", json.dumps(subject)),
        ],
    )
    record = client._send("Decide", {"request": asked})["records"][0]
    facts = [p["fact"] for p in record["premises"]]
    assert "promotion.verdict=promote" in facts, facts
    return client._send("DecisionLog", {"op": dc.commit_op(record)})["record_id"]
