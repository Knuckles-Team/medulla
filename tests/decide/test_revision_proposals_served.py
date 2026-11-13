"""MEDULLA-DECISION-ENGINE-R817 served receipts: the revision proposal list is
tenant scoped and paged over the wire.

Rows come from the engine's own writer: a harness adoption (R803.8) stores an
``evaluated`` revision of the agent's harness bundle in the same owner write.
Two adoptions by the first tenant give two rows; a second tenant's list never
shows them, and its use of the first tenant's cursor reads as an empty page.
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
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.models import HarnessAdoptionStored, RevisionProposalPage

OTHER_TENANT = "tenant:other-revisions"
FIRST_AGENT = "agent:r817-first"
SECOND_AGENT = "agent:r817-second"
BAD_CURSOR = "not-a-cursor"


def _write(record_digest: str, adopted_ref: str, agent_ref: str, trace: str) -> dict:
    return {
        "task_ref": TASK_REF,
        "task_class": TASK_CLASS,
        "agent_ref": agent_ref,
        "decision_record_digest": record_digest,
        "conditions_digest": CONDITIONS_DIGEST,
        "adopted_version_ref": adopted_ref,
        "deficiency": "retries",
        "situation": "a run that retried its tool call",
        "changed_component_kind": "system_prompt",
        "what_changed": "the prompt names the tool contract",
        "applicability": ["tool_error"],
        "confidence_ppm": 800_000,
        "evidence_run_ids": [BASELINE, trace],
    }


async def _journey(
    writes: list[dict],
) -> tuple[list[dict], list[dict], dict, dict, dict]:
    """Adopt as the first tenant, then list as each tenant over the wire."""
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    owner = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    stranger = await MedullaClient.connect(
        socket_path=socket_path,
        verified_context=dict(request_context(), tenant=OTHER_TENANT),
    )
    try:
        mine = dc.DecisionClient(client=owner)
        theirs = dc.DecisionClient(client=stranger)
        stored = [
            await mine.log(dc.harness_adoption_op(TEST_TENANT, **write))
            for write in writes
        ]
        pages = []
        cursor = None
        while True:
            page = await mine.revision_proposal_list(cursor=cursor, limit=1)
            pages.append(page)
            cursor = page["next_cursor"]
            if cursor is None:
                break
        everything = await mine.revision_proposal_list()
        foreign = await theirs.revision_proposal_list()
        foreign_with_cursor = await theirs.revision_proposal_list(
            cursor=pages[0]["next_cursor"]
        )
        with pytest.raises(EngineResponseError, match="PROPOSAL_CURSOR_INVALID"):
            await mine.revision_proposal_list(cursor=BAD_CURSOR)
        return stored, pages, everything, foreign, foreign_with_cursor
    finally:
        await owner.close()
        await stranger.close()


# spec: MEDULLA-DECISION-ENGINE-R817
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R817")
def test_list_is_tenant_scoped_and_paged(clean_graph):
    improving_ref = version_node_id(COMPONENT_A, "blob:r817-improving")
    improving_trace = "trace:r817-improving"
    improving = {
        "retry_delta": -Q32,
        "tool_selection_delta": 0,
        "eval_score_delta": 2 * Q32,
        "cost_delta": -Q32,
    }
    _apply_evidence(
        clean_graph,
        "h2-r817:evidence",
        [
            (
                improving_ref,
                version_props(COMPONENT_A, "blob:r817-improving", 10, 100),
            ),
            (probe_run_node_id(BASELINE), probe_run_props(BASELINE, 102)),
            (probe_run_node_id(improving_trace), probe_run_props(improving_trace, 103)),
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
        clean_graph, "h2-r817-schema", "feature_schema", contrast_schema_body()
    )
    options = [candidate(improving_ref, improving_trace, **improving)]
    _, record, _, _ = asyncio.run(
        _contrast_journey(schema, options, "q:h2-r817:adopt", _graph_name(clean_graph))
    )
    assert record["inputs"]["harness_contrast"]["adopted_version_ref"] == improving_ref

    writes = [
        _write(record["record_digest"], improving_ref, agent, improving_trace)
        for agent in (FIRST_AGENT, SECOND_AGENT)
    ]
    stored, pages, everything, foreign, foreign_with_cursor = asyncio.run(
        _journey(writes)
    )
    adopted = [
        HarnessAdoptionStored.model_validate(item).adopted_harness_ref
        for item in stored
    ]

    # Limit 1 pages the two rows one at a time, in creation order, and the last
    # page carries no cursor.
    parsed = [RevisionProposalPage.model_validate(page) for page in pages]
    assert [len(page.rows) for page in parsed] == [1, 1]
    assert parsed[0].next_cursor is not None
    assert parsed[1].next_cursor is None
    paged_ids = [row.id for page in parsed for row in page.rows]
    assert paged_ids == adopted

    # The unpaged list is the same rows; each is the evaluated revision of the
    # agent's harness bundle with its evaluation receipt.
    full = RevisionProposalPage.model_validate(everything)
    assert [row.id for row in full.rows] == adopted
    for row in full.rows:
        assert row.stage == "evaluated"
        assert row.subject_kind == "definition"
        assert row.subject_ref.endswith(f":{TASK_CLASS}")
        assert row.digest == record["record_digest"]
        assert row.evaluation_receipt_id

    # Another tenant sees none of them, with or without the first tenant's cursor.
    assert RevisionProposalPage.model_validate(foreign).rows == []
    assert RevisionProposalPage.model_validate(foreign_with_cursor).rows == []
