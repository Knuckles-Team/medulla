"""MEDULLA-DECISION-ENGINE-R711.6 served skill evaluation.

L4: every call goes through the running engine over the wire client. One suite holds 1
golden, 16 real-prompt and 3 author-judged cases. The job runs with a judge calibration
of 0.7 and answers band ``healthy``, reruns with every case passing and answers
``saturated`` with the suspicion premise, and reruns with a calibration of 0.4 and
refuses the judged cases with ``JUDGE_NOT_CALIBRATED``.
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
TRAJECTORY = [{"kind": "tool", "name": "search"}]


def _suite(skill: str) -> list[dict[str, Any]]:
    cases = []
    for index in range(20):
        kind = (
            "golden"
            if index == 0
            else "real_prompt"
            if index <= 16
            else "author_judged"
        )
        cases.append(
            {
                "case_id": f"{skill}-case-{index}",
                "skill_ref": skill,
                "kind": kind,
                "input_digest": f"in-{index}",
                "expected_digest": f"out-{index}",
                "expected_trajectory": TRAJECTORY if kind == "real_prompt" else [],
                "source": "authored",
            }
        )
    return cases


def _runs(cases: list[dict[str, Any]], failing: int) -> list[dict[str, Any]]:
    runs = []
    for index, case in enumerate(cases):
        ok = index >= failing
        runs.append(
            {
                "case_id": case["case_id"],
                "actual_digest": f"out-{index}" if ok else "wrong",
                "actual_trajectory": TRAJECTORY if ok else [],
                "judged_passed": ok,
                "attestation": {
                    "sandbox_digest": "sandbox",
                    "env_digest": "env",
                    "host_paths_read": 0,
                },
                "run_trace_id": f"trace-{index}",
            }
        )
    return runs


def _request(skill: str, failing: int, kappa_ppm: int) -> dict[str, Any]:
    cases = _suite(skill)
    return {
        "tenant_id": request_context()["tenant"],
        "skill_ref": skill,
        "cases": cases,
        "runs": _runs(cases, failing),
        "calibration": {
            "judge_ref": "judge-1",
            "annotated_set_ref": "set-1",
            "kappa_ppm": kappa_ppm,
            "n": 60,
        },
        "known_tools": ["search"],
    }


async def _journey(skill: str) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    try:
        decisions = dc.DecisionClient(client=client, graph=GRAPH)
        healthy = await decisions.skill_eval(_request(skill, 4, 700_000))
        saturated = await decisions.skill_eval(_request(skill, 0, 700_000))
        uncalibrated = await decisions.skill_eval(_request(skill, 0, 400_000))
        return {
            "healthy": healthy,
            "saturated": saturated,
            "uncalibrated": uncalibrated,
        }
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R711
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R711.6")
def test_mix_band_and_judge_calibration(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R711.6
    skill = f"skill:eval{uuid.uuid4().hex[:8]}@1"
    out = asyncio.run(_journey(skill))

    assert out["healthy"]["report"]["band"] == "healthy"
    assert "eval:mix=1/16/3" in out["healthy"]["premises"]

    assert out["saturated"]["report"]["band"] == "saturated"
    assert "eval:suspicious_saturation" in out["saturated"]["premises"]

    refused = {row["case_id"]: row["code"] for row in out["uncalibrated"]["refused"]}
    assert len(refused) == 3
    assert set(refused.values()) == {"JUDGE_NOT_CALIBRATED"}
