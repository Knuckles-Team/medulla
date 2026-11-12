"""MEDULLA-DECISION-ENGINE-R700.3 served belief revision: a pass over claims
whose evidence changed, through the ``belief_revise`` client method.

Two claims take opposing evidence, a pass revises both, reports the pair as a
tracked contradiction, and refuses an unknown claim.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

GRAPH = "default"


async def _pass(tag: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        higher = f"cache-hit-rate-is-higher-{tag}"
        lower = f"cache-hit-rate-is-lower-{tag}"
        for claim_id, supports in ((higher, True), (lower, False)):
            await decision.claims(dc.claim_propose_op(claim_id))
            await decision.claims(
                dc.claim_evidence_op(
                    f"{tag}:{claim_id}",
                    claim_id,
                    "outcome",
                    supports=supports,
                    weight_ppm=1_000_000,
                    source_digest=f"sha256:{tag:0<64}",
                )
            )
        revised = await decision.belief_revise(
            dc.belief_revise_request(
                GRAPH,
                [higher, lower],
                texts={
                    higher: "the cache hit rate is higher",
                    lower: "the cache hit rate is lower",
                },
            )
        )
        try:
            await decision.belief_revise(
                dc.belief_revise_request(GRAPH, [f"missing-{tag}"])
            )
            missing = None
        except Exception as error:  # the typed refusal carries the code
            missing = str(error)
        return {
            "higher": higher,
            "lower": lower,
            "revised": revised,
            "missing": missing,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R700.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R700.3")
def test_pass_served(clean_graph):
    outcome = asyncio.run(_pass(uuid.uuid4().hex[:12]))
    revised = outcome["revised"]

    by_claim = {r["claim_id"]: r for r in revised["revisions"]}
    assert set(by_claim) == {outcome["higher"], outcome["lower"]}
    assert by_claim[outcome["higher"]]["posterior_micro"] > 0
    assert by_claim[outcome["lower"]]["posterior_micro"] < 0
    assert revised["claims_visited"] == 2
    assert len(revised["pass_id"]) == 32

    pairs = {(c["left"], c["right"]) for c in revised["contradictions"]}
    assert (
        min(outcome["higher"], outcome["lower"]),
        max(outcome["higher"], outcome["lower"]),
    ) in pairs
    assert all(c["status"] == "open" for c in revised["contradictions"])

    assert outcome["missing"] is not None
    assert "BELIEF_CLAIM_NOT_FOUND" in outcome["missing"]
