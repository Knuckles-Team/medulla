"""MEDULLA-DECISION-ENGINE-R817 served receipts: the revision proposal list is
tenant scoped and paged over the wire.

Rows come from the engine's own writer: a harness adoption (R803.8) stores an
``evaluated`` revision of the agent's harness bundle in the same owner write.
Two adoptions by the deployment tenant give two rows, paged one at a time. A
deployed engine admits one tenant, so a context naming another tenant is
refused at the door with ``AUTH_TENANT_MISMATCH``; isolation between two
tenants in one store is proven by the store-level test
``revision_proposal_tests.rs::list_is_tenant_scoped_and_paged`` (L3).
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


async def _journey(writes: list[dict]) -> tuple[list[str], list[dict], list[dict], dict]:
    """Adopt and list as the deployment tenant; a foreign tenant is refused.

    The deployment tenant is shared by the session's served tests, so the rows
    it already holds are read first and returned beside the new ones.
    """
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
        before = [row["id"] for row in (await mine.revision_proposal_list())["rows"]]
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
        with pytest.raises(EngineResponseError, match="PROPOSAL_CURSOR_INVALID"):
            await mine.revision_proposal_list(cursor=BAD_CURSOR)
        with pytest.raises(EngineResponseError, match="AUTH_TENANT_MISMATCH"):
            await theirs.revision_proposal_list()
        return before, stored, pages, everything
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
    # The contrast policy decides over at least two declared candidates
    # (R803 knobs), so a neutral candidate of another component is held too.
    neutral_ref = version_node_id(COMPONENT_B, "blob:r817-neutral")
    neutral_trace = "trace:r817-neutral"
    neutral = {
        "retry_delta": 0,
        "tool_selection_delta": 0,
        "eval_score_delta": 0,
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
            (neutral_ref, version_props(COMPONENT_B, "blob:r817-neutral", 10, 101)),
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
        clean_graph, "h2-r817-schema", "feature_schema", contrast_schema_body()
    )
    options = [
        candidate(improving_ref, improving_trace, **improving),
        candidate(neutral_ref, neutral_trace, **neutral),
    ]
    # One decision record adopts once (a repeat of its write is a replay), so
    # each agent's adoption is decided by its own record.
    records = []
    for agent in (FIRST_AGENT, SECOND_AGENT):
        _, record, _, _ = asyncio.run(
            _contrast_journey(
                schema, options, f"q:h2-r817:adopt:{agent}", _graph_name(clean_graph)
            )
        )
        adopted_ref = record["inputs"]["harness_contrast"]["adopted_version_ref"]
        assert adopted_ref == improving_ref
        records.append((agent, record))
    assert records[0][1]["record_digest"] != records[1][1]["record_digest"]

    writes = [
        _write(record["record_digest"], improving_ref, agent, improving_trace)
        for agent, record in records
    ]
    before, stored, pages, everything = asyncio.run(_journey(writes))
    adopted = [
        HarnessAdoptionStored.model_validate(item).adopted_harness_ref
        for item in stored
    ]

    # Limit 1 pages every row one at a time, in creation order: the rows the
    # tenant held before, then the two new ones; the last page carries no cursor.
    parsed = [RevisionProposalPage.model_validate(page) for page in pages]
    assert [len(page.rows) for page in parsed] == [1] * (len(before) + 2)
    assert all(page.next_cursor is not None for page in parsed[:-1])
    assert parsed[-1].next_cursor is None
    paged_ids = [row.id for page in parsed for row in page.rows]
    assert paged_ids == before + adopted

    # The unpaged list is the same rows; each is the evaluated revision of the
    # agent's harness bundle with its evaluation receipt.
    full = RevisionProposalPage.model_validate(everything)
    assert [row.id for row in full.rows] == before + adopted
    for row, (_, record) in zip(full.rows[len(before) :], records, strict=True):
        assert row.stage == "evaluated"
        assert row.subject_kind == "definition"
        assert row.subject_ref.endswith(f":{TASK_CLASS}")
        assert row.digest == record["record_digest"]
        assert row.evaluation_receipt_id
