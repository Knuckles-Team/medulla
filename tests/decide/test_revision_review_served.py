"""MEDULLA-DECISION-ENGINE-R805.15 served receipts: an operator reviews an ``evaluated``
revision proposal over ``DecisionLog``.

The proposals come from the engine's own writer: a harness adoption (R803.8) stores
an ``evaluated`` revision authored by the adopting principal. A second principal
reviews it. Approve publishes the revision, veto retires it, a repeat of one review
answers the stored receipt, and a stale digest, the author as reviewer and a row in
any other stage each refuse with their typed code.
"""

from __future__ import annotations

import asyncio
import os

import pytest
from _decision_resident_fixture import publish
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT, request_context

from decide.test_harness_contrast_served import (
    COMPONENT_A,
    COMPONENT_B,
    ProbeRun,
    _contrast_journey,
    candidate,
    contrast_schema_body,
    seed_probe_runs,
    version_node_id,
    version_props,
)
from decide.test_revision_proposals_served import _write
from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.models import HarnessAdoptionStored, RevisionProposalPage

APPROVED_AGENT = "agent:r805-15-approved"
VETOED_AGENT = "agent:r805-15-vetoed"
REVIEWER = "agent:r805-15-reviewer"
REASON = "the contrast evidence holds"


async def _review_journey(writes: list[dict]) -> dict:
    """Adopt as the deployment principal, then review as a second principal."""
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    author = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    reviewer = await MedullaClient.connect(
        socket_path=socket_path,
        verified_context=request_context(agent_id=REVIEWER, principal=REVIEWER),
    )
    try:
        mine = dc.DecisionClient(client=author)
        theirs = dc.DecisionClient(client=reviewer)
        stored = [
            HarnessAdoptionStored.model_validate(
                await mine.log(dc.harness_adoption_op(TEST_TENANT, **write))
            )
            for write in writes
        ]

        async def row_of(revision_id: str):
            page = RevisionProposalPage.model_validate(
                await theirs.revision_proposal_list()
            )
            return next(row for row in page.rows if row.id == revision_id)

        approve_id = stored[0].adopted_harness_ref
        veto_id = stored[1].adopted_harness_ref
        approve_row = await row_of(approve_id)
        veto_row = await row_of(veto_id)
        assert approve_row.stage == "evaluated"
        assert veto_row.stage == "evaluated"

        async def review(who, revision_id, decision, digest, reason=REASON):
            return await who.review_revision(
                TEST_TENANT, revision_id, decision, digest, reason
            )

        refusals = {}
        for name, call in {
            "REVISION_NOT_FOUND": review(theirs, "rev:absent", "approve", "sha256:x"),
            "REVISION_DIGEST_MISMATCH": review(
                theirs, approve_id, "approve", "sha256:stale"
            ),
            "REVIEW_SELF_APPROVAL": review(
                mine, approve_id, "approve", approve_row.digest
            ),
            "PARAMETER_INVALID": review(
                theirs, approve_id, "approve", approve_row.digest, ""
            ),
        }.items():
            with pytest.raises(EngineResponseError, match=name):
                await call
            refusals[name] = True
        assert (await row_of(approve_id)).stage == "evaluated"

        approved = await review(theirs, approve_id, "approve", approve_row.digest)
        repeat = await review(theirs, approve_id, "approve", approve_row.digest)
        after_approve = await row_of(approve_id)
        with pytest.raises(EngineResponseError, match="TRANSITION_REFUSED"):
            await review(theirs, approve_id, "veto", approve_row.digest)
        vetoed = await review(theirs, veto_id, "veto", veto_row.digest)
        after_veto = await row_of(veto_id)
        return {
            "approved": approved,
            "repeat": repeat,
            "vetoed": vetoed,
            "after_approve": after_approve,
            "after_veto": after_veto,
            "approve_id": approve_id,
            "veto_id": veto_id,
            "approve_digest": approve_row.digest,
            "veto_digest": veto_row.digest,
            "refusals": refusals,
        }
    finally:
        await author.close()
        await reviewer.close()


# spec: MEDULLA-DECISION-ENGINE-R805.15
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R805.15")
def test_review_refuses_stale_digest_self_approval_and_wrong_stage(clean_graph):
    improving_ref = version_node_id(COMPONENT_A, "blob:r805-15-improving")
    neutral_ref = version_node_id(COMPONENT_B, "blob:r805-15-neutral")
    ids = seed_probe_runs(
        clean_graph,
        "h2-r805-15:evidence",
        [
            (
                improving_ref,
                version_props(COMPONENT_A, "blob:r805-15-improving", 10, 100),
            ),
            (neutral_ref, version_props(COMPONENT_B, "blob:r805-15-neutral", 10, 101)),
        ],
        [
            ProbeRun("baseline", retries=1, extra_ok=0, score=0.25, cost=3),
            ProbeRun("improving", retries=0, extra_ok=1, score=0.75, cost=2),
            ProbeRun("neutral", retries=1, extra_ok=0, score=0.25, cost=2),
        ],
    )
    baseline, improving_trace, neutral_trace = (
        ids["baseline"],
        ids["improving"],
        ids["neutral"],
    )
    schema = publish(
        clean_graph, "h2-r805-15-schema", "feature_schema", contrast_schema_body()
    )
    options = [
        candidate(improving_ref, improving_trace),
        candidate(neutral_ref, neutral_trace),
    ]
    records = []
    for agent in (APPROVED_AGENT, VETOED_AGENT):
        _, record, _, _ = asyncio.run(
            _contrast_journey(
                schema,
                options,
                f"q:h2-r805-15:adopt:{agent}",
                _graph_name(clean_graph),
                baseline,
            )
        )
        records.append((agent, record))
    writes = [
        _write(record["record_digest"], improving_ref, agent, baseline, improving_trace)
        for agent, record in records
    ]
    out = asyncio.run(_review_journey(writes))

    assert set(out["refusals"]) == {
        "REVISION_NOT_FOUND",
        "REVISION_DIGEST_MISMATCH",
        "REVIEW_SELF_APPROVAL",
        "PARAMETER_INVALID",
    }
    approve_receipt = f"review:{out['approve_id']}:{out['approve_digest']}"
    assert out["approved"] == {
        "revision_id": out["approve_id"],
        "stage": "published",
        "receipt_ref": approve_receipt,
    }
    assert out["repeat"] == out["approved"]
    assert out["after_approve"].stage == "published"
    assert out["vetoed"] == {
        "revision_id": out["veto_id"],
        "stage": "retired",
        "receipt_ref": f"review:{out['veto_id']}:{out['veto_digest']}",
    }
    assert out["after_veto"].stage == "retired"
