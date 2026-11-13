"""MEDULLA-DECISION-ENGINE-R803.8 served receipts: the harness adoption, its
contrast and its lesson stored through ``DecisionLog``, then the lesson read
back through the served ``harness_lessons`` read.

The journey runs over the wire: the evidence nodes are written through the
generated client write, the ``harness_contrast`` decision is decided and
committed (R803.3), and the ``harness_adoption`` op names that committed record
by digest. The request carries no contrast delta; the engine reads it from the
committed decision.
"""

from __future__ import annotations

import asyncio
import os

import pytest
from _decision_resident_fixture import publish
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from decide.test_harness_contrast_served import (
    BASELINE,
    COMPONENT_A,
    COMPONENT_B,
    CONDITIONS_DIGEST,
    Q32,
    TASK_CLASS,
    TASK_REF,
    _apply_evidence,
    _contrast_journey,
    candidate,
    contrast_node_id,
    contrast_props,
    contrast_schema_body,
    probe_run_node_id,
    probe_run_props,
    version_node_id,
    version_props,
)
from medulla import decision_client as dc
from medulla.client import MedullaClient
from medulla.generated.models import HarnessAdoptionStored, HarnessLessonsFound

AGENT_REF = "agent:h2-r803-8"
DEFICIENCY = "retries"
FAR_FUTURE_MS = 2**52


async def _adopt_and_read(
    write: dict, features: list[str]
) -> tuple[dict, dict, dict, dict]:
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    client = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    try:
        decision = dc.DecisionClient(client=client)
        before = await decision.log(
            dc.harness_lessons_op(
                TEST_TENANT, TASK_CLASS, "unlearned-deficiency", FAR_FUTURE_MS
            )
        )
        stored = await decision.log(dc.harness_adoption_op(TEST_TENANT, **write))
        repeat = await decision.log(dc.harness_adoption_op(TEST_TENANT, **write))
        found = await decision.log(
            dc.harness_lessons_op(
                TEST_TENANT, TASK_CLASS, DEFICIENCY, FAR_FUTURE_MS, features
            )
        )
        return before, stored, repeat, found
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R803.8
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R803.8")
def test_adoption_logged_and_lesson_retrieved(clean_graph):
    improving_ref = version_node_id(COMPONENT_A, "blob:adopt-improving")
    neutral_ref = version_node_id(COMPONENT_B, "blob:adopt-neutral")
    improving_trace = "trace:adopt-improving"
    neutral_trace = "trace:adopt-neutral"
    improving = {
        "retry_delta": -Q32,
        "tool_selection_delta": 0,
        "eval_score_delta": 2 * Q32,
        "cost_delta": -Q32,
    }
    neutral = {
        "retry_delta": 0,
        "tool_selection_delta": 0,
        "eval_score_delta": 0,
        "cost_delta": -Q32,
    }
    _apply_evidence(
        clean_graph,
        "h2-r803-8:evidence",
        [
            (
                improving_ref,
                version_props(COMPONENT_A, "blob:adopt-improving", 10, 100),
            ),
            (neutral_ref, version_props(COMPONENT_B, "blob:adopt-neutral", 10, 101)),
            (probe_run_node_id(BASELINE), probe_run_props(BASELINE, 102)),
            (probe_run_node_id(improving_trace), probe_run_props(improving_trace, 103)),
            (probe_run_node_id(neutral_trace), probe_run_props(neutral_trace, 104)),
            (
                contrast_node_id(BASELINE, improving_trace),
                contrast_props(
                    BASELINE,
                    improving_trace,
                    improving["retry_delta"],
                    improving["tool_selection_delta"],
                    improving["eval_score_delta"],
                    improving["cost_delta"],
                    105,
                ),
            ),
        ],
    )
    schema = publish(
        clean_graph, "h2-r803-8-schema", "feature_schema", contrast_schema_body()
    )
    options = [
        candidate(improving_ref, improving_trace, **improving),
        candidate(neutral_ref, neutral_trace, **neutral),
    ]
    _, record, _, _ = asyncio.run(
        _contrast_journey(
            schema, options, "q:h2-r803-8:adopt", _graph_name(clean_graph)
        )
    )
    assert record["inputs"]["harness_contrast"]["adopted_version_ref"] == improving_ref

    write = {
        "task_ref": TASK_REF,
        "task_class": TASK_CLASS,
        "agent_ref": AGENT_REF,
        "decision_record_digest": record["record_digest"],
        "conditions_digest": CONDITIONS_DIGEST,
        "adopted_version_ref": improving_ref,
        "deficiency": DEFICIENCY,
        "situation": "a run that retried its tool call",
        "changed_component_kind": "system_prompt",
        "what_changed": "the prompt names the tool contract",
        "applicability": ["tool_error"],
        "confidence_ppm": 800_000,
        "evidence_run_ids": [BASELINE, improving_trace],
    }
    before, stored, repeat, found = asyncio.run(_adopt_and_read(write, ["tool_error"]))

    # A well-formed read nothing matches abstains with the registered code.
    abstained = HarnessLessonsFound.model_validate(before)
    assert abstained.lessons == []
    assert abstained.abstain_reason == "NO_LESSONS"

    # The adoption stores once; the identical repeat answers the same refs.
    first = HarnessAdoptionStored.model_validate(stored)
    again = HarnessAdoptionStored.model_validate(repeat)
    assert again.replayed is True
    assert again.adopted_harness_ref == first.adopted_harness_ref
    assert again.lesson_ref == first.lesson_ref

    # The lesson reads back through the served read, with its evidence.
    result = HarnessLessonsFound.model_validate(found)
    assert result.abstain_reason is None
    lesson = next(item for item in result.lessons if item.deficiency == DEFICIENCY)
    assert lesson.task_class == TASK_CLASS
    assert lesson.what_changed == write["what_changed"]
    assert lesson.evidence_run_ids == [BASELINE, improving_trace]
    assert lesson.confidence == pytest.approx(0.8)
