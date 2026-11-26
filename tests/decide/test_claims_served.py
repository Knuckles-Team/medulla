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


STANDARD_AXIOMS = ["Classical.choice", "Quot.sound", "propext"]


def _receipt(
    declaration: str,
    axioms: list[str],
    *,
    has_sorry: bool = False,
    verdict: str = "pass",
) -> dict[str, Any]:
    return {
        "checker_id": "kernel-checker",
        "toolchain_version": "1.2.3",
        "declaration": declaration,
        "artifact_digest": "ab" * 32,
        "axioms": axioms,
        "has_sorry": has_sorry,
        "verdict": verdict,
    }


async def _record_receipt(
    decision: dc.DecisionClient,
    tag: str,
    claim_id: str,
    name: str,
    receipt: dict[str, Any],
) -> dict[str, Any]:
    return await decision.claims(
        dc.claim_evidence_op(
            f"{tag}:{name}",
            claim_id,
            "process",
            supports=True,
            weight_ppm=900_000,
            source_digest=f"sha256:{name}",
            proof=receipt,
        )
    )


async def _proof_journey(tag: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        proven_id, extra_id, sorry_id = (
            f"claim:{tag}:{name}" for name in ("proven", "extra", "sorry")
        )
        for claim_id in (proven_id, extra_id, sorry_id):
            await decision.claims(dc.claim_propose_op(claim_id))
        recorded = await _record_receipt(
            decision, tag, proven_id, "p1", _receipt("Mathlib.A.b", STANDARD_AXIOMS)
        )
        await _record_receipt(
            decision, tag, extra_id, "x1", _receipt("Mathlib.C.d", ["Foo.axiom"])
        )
        refusals: dict[str, str] = {}
        for label, receipt in (
            ("sorry", _receipt("Mathlib.E.f", [], has_sorry=True)),
            ("invalid", {**_receipt("Mathlib.G.h", []), "artifact_digest": "xyz"}),
        ):
            try:
                await _record_receipt(decision, tag, sorry_id, label, receipt)
            except Exception as error:  # the typed refusal carries the code
                refusals[label] = str(error)
        read = await decision.claims(dc.claim_get_op(claim_id=proven_id))
        unchanged = await decision.claims(dc.claim_get_op(claim_id=sorry_id))
        listed = await decision.claims(dc.claim_proven_op(limit=1000))
        filtered = await decision.claims(
            dc.claim_proven_op(axiom="Foo.axiom", limit=1000)
        )
        try:
            await decision.claims(dc.claim_proven_op(limit=1001))
            too_many = None
        except Exception as error:
            too_many = str(error)
        return {
            "recorded": recorded,
            "read": read,
            "unchanged": unchanged,
            "listed": listed,
            "filtered": filtered,
            "refusals": refusals,
            "too_many": too_many,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R652.13
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R652.13")
def test_proven_claims_served(clean_graph):
    tag = uuid.uuid4().hex[:12]
    outcome = asyncio.run(_proof_journey(tag))

    mine = [
        row
        for row in outcome["listed"]["claims"]
        if row["claim_id"].startswith(f"claim:{tag}:")
    ]
    # One row per claim and receipt: the sorry receipt is not listed, the
    # non-standard axiom is listed as unproven, rows sort by claim id.
    assert [(row["claim_id"], row["standing"]) for row in mine] == [
        (f"claim:{tag}:extra", "unproven"),
        (f"claim:{tag}:proven", "proven"),
    ]
    assert mine[1]["axioms"] == STANDARD_AXIOMS
    assert mine[0]["axioms"] == ["Foo.axiom"]
    only_foo = [
        row
        for row in outcome["filtered"]["claims"]
        if row["claim_id"].startswith(f"claim:{tag}:")
    ]
    assert [row["claim_id"] for row in only_foo] == [f"claim:{tag}:extra"]
    assert outcome["too_many"] is not None
    assert "PARAMETER_INVALID" in outcome["too_many"]


# spec: MEDULLA-DECISION-ENGINE-R652.14
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R652.14")
def test_proof_standing_served(clean_graph):
    tag = uuid.uuid4().hex[:12]
    outcome = asyncio.run(_proof_journey(tag))

    assert outcome["recorded"]["proof_standing"] == "proven"
    assert outcome["read"]["claim"]["proof_standing"] == "proven"
    assert "PROOF_SORRY_PRESENT" in outcome["refusals"]["sorry"]
    assert "PROOF_RECEIPT_INVALID" in outcome["refusals"]["invalid"]
    # The refused receipts left the claim unproven and unrecorded.
    claim = outcome["unchanged"]["claim"]
    assert claim["proof_standing"] == "unproven"
    assert claim["evidence"] == []
