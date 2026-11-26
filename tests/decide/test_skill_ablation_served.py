"""MEDULLA-DECISION-ENGINE-R712.6 served skill ablation.

L4: every call goes through the running engine over the wire client. Five identical
runs set the noise band of a skill. 200 paired with-and-without runs that differ by
100000 ppm show an ``effect`` outside the band, and a second record whose arms differ
by 10 ppm shows ``within_noise``. A canary is rolled back, and a canary with no
champion is refused. A change of the model revision marks the other compiled row of
the model ``stale`` and files one proposal per stale row.
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

GRAPH = "__commons__"


def _runs(skill: str, gain: int) -> list[dict[str, Any]]:
    runs = []
    for index in range(200):
        for arm, score in (("with", 600_000 + gain), ("without", 600_000)):
            runs.append(
                {
                    "skill_ref": skill,
                    "prompt_digest": f"prompt-{index}",
                    "arm": arm,
                    "blind_id": f"blind-{index}-{arm}",
                    "outcome_score_ppm": score,
                    "seed": index % 3,
                }
            )
    return runs


async def _journey(prefix: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    try:
        tenant = request_context()["tenant"]
        skill = f"skill:{prefix}-triage@3"
        model = f"model:{prefix}"
        await client.lifecycle.batch_update(
            [{"op": "upsert_node", "id": model, "properties": {"type": "Model"}}]
        )
        decisions = dc.DecisionClient(client=client, graph=GRAPH)

        async def learn(write: str, **fields: Any) -> dict[str, Any]:
            return await decisions.log(dc.learn_op(tenant, write, **fields))

        identical = [700_000, 702_000, 698_000, 701_000, 699_000]
        effect = await learn(
            "record_ablation",
            record={
                "skill_ref": skill,
                "identical_scores_ppm": identical,
                "runs": _runs(skill, 100_000),
            },
        )
        within = await learn(
            "record_ablation",
            record={
                "skill_ref": skill,
                "identical_scores_ppm": identical,
                "runs": _runs(skill, 10),
            },
        )
        canary = {
            "skill_ref": skill,
            "champion_revision": "revision:one",
            "challenger_revision": "revision:two",
            "traffic_ppm": 50_000,
            "window_ms": 3_600_000,
        }
        # A canary needs a champion: the skill has none here, so the write refuses.
        refused = None
        try:
            await learn("record_canary", canary=canary, verdict="ramp")
        except Exception as error:
            refused = str(error)
        rolled_back = await learn("record_canary", canary=canary, verdict="rollback")

        stale = None
        for other, revision in (
            (skill, "r1"),
            (f"{skill}-b", "r1"),
            (f"{skill}-c", "r2"),
        ):
            stale = await learn(
                "record_model_compile",
                graph=GRAPH,
                compile={
                    "skill_ref": other,
                    "model_ref": model,
                    "compiled_digest": f"digest-{other}",
                    "state": "current",
                },
                model_revision=revision,
            )
        return {
            "effect": effect,
            "within": within,
            "refused": refused,
            "rolled_back": rolled_back,
            "stale": stale,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R712
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R712.6")
def test_effect_noise_canary_and_stale_compile(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R712.6
    out = asyncio.run(_journey("ab" + uuid.uuid4().hex[:8]))
    assert out["effect"]["noise"]["identical_runs"] == 5
    assert out["effect"]["verdict"] == {"effect": {"delta_ppm": 100_000}}
    assert out["within"]["verdict"] == "within_noise"
    assert out["refused"] is not None and "CANARY_NO_CHAMPION" in out["refused"]
    assert out["rolled_back"]["canary"] is None
    assert len(out["stale"]["stale"]) == 2
    assert len(out["stale"]["proposals"]) == 2
    assert all(row["state"] == "stale" for row in out["stale"]["stale"])
