"""MEDULLA-DECISION-ENGINE-R652.3/.6 served claims: the claim lifecycle over
the wire through the ``claims`` client method.

A claim is proposed, takes evidence on all five channels, advances to
``accepted`` once the log-odds clear the guards, compiles into a sealed
research artifact that reads back, and refuses an illegal edge and a
cross-tenant read.
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

CHANNELS = ["outcome", "optimization", "health", "process", "mined"]
#: Cumulative log-odds in micro-nats after each channel at full weight, from
#: the default channel priors.
LOG_ODDS = [1_000_000, 1_800_000, 2_400_000, 2_900_000, 3_200_000]


async def _journey(tag: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        claim_id = f"claim:{tag}"
        proposed = await decision.claims(dc.claim_propose_op(claim_id))
        states = []
        for index, channel in enumerate(CHANNELS):
            record = await decision.claims(
                dc.claim_evidence_op(
                    f"{tag}:e{index}",
                    claim_id,
                    channel,
                    supports=True,
                    weight_ppm=1_000_000,
                    source_digest=f"sha256:{index:064x}",
                    stances={"alpha": "supports", "beta": "opposes"},
                )
            )
            states.append(record["log_odds_micro"])
        await decision.claims(dc.claim_advance_op(claim_id, "supported"))
        accepted = await decision.claims(dc.claim_advance_op(claim_id, "accepted"))
        artifact_id = f"artifact:{tag}"
        compiled = await decision.claims(
            dc.claim_compile_op(artifact_id, f"cohort:{tag}", [claim_id])
        )
        read_back = await decision.claims(dc.claim_get_op(artifact_id=artifact_id))
        claim = await decision.claims(dc.claim_get_op(claim_id=claim_id))
        try:
            await decision.claims(dc.claim_advance_op(claim_id, "proposed"))
            illegal = None
        except Exception as error:  # the typed refusal carries the code
            illegal = str(error)
        try:
            await decision.claims(
                dc.claim_evidence_op(
                    f"{tag}:none",
                    claim_id,
                    None,
                    supports=True,
                    weight_ppm=1,
                    source_digest="sha256:0",
                )
            )
            no_channel = None
        except Exception as error:
            no_channel = str(error)
        return {
            "proposed": proposed,
            "log_odds": states,
            "accepted": accepted,
            "compiled": compiled,
            "read_back": read_back,
            "claim": claim,
            "illegal": illegal,
            "no_channel": no_channel,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R652.3
# spec: MEDULLA-DECISION-ENGINE-R652.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R652.3")
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R652.6")
def test_claim_lifecycle_served(clean_graph):
    outcome = asyncio.run(_journey(uuid.uuid4().hex[:12]))

    assert outcome["proposed"]["state"] == "proposed"
    assert outcome["log_odds"] == LOG_ODDS
    assert outcome["accepted"]["state"] == "accepted"
    assert [t["to"] for t in outcome["accepted"]["transitions"]] == [
        "supported",
        "accepted",
    ]

    seal = outcome["compiled"]["seal"]["digest"]
    assert len(seal) == 64 and seal == seal.lower()
    assert outcome["read_back"]["artifact"] == outcome["compiled"]
    assert outcome["claim"]["claim"]["contradiction_pairs"] == [["alpha", "beta"]]
    assert len(outcome["claim"]["claim"]["evidence"]) == len(CHANNELS)

    assert outcome["illegal"] is not None
    assert "CLAIM_ILLEGAL_TRANSITION" in outcome["illegal"]
    assert outcome["no_channel"] is not None
    assert "EVIDENCE_NO_CHANNEL" in outcome["no_channel"]
