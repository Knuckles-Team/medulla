"""MEDULLA-SDLC-R013: scenario runs are records of the one decision log, served.

L4: every call goes through the running engine's ``DecisionLog``. A run is committed
once, a repeat replays, an incomplete record refuses ``SCENARIO_RUN_INCOMPLETE``, and
the read lists the latest run of a phase first so the bundle rule can take the latest
one on ``main``.
"""

from __future__ import annotations

import asyncio
import os

import pytest
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient


def _run(phase: str, commit: str, result: str, **overrides) -> dict:
    record = {
        "phase": phase,
        "scenario_id": "scenario:journey",
        "commit": commit,
        "branch": "main",
        "result": result,
        "requirement_ids": ["DEMO-R001.1"],
        "evidence_digests": ["sha256:" + "a" * 64],
    }
    record.update(overrides)
    return record


async def _journey() -> dict:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        phase = "phase-served-r013"
        failed = await decision.log(
            dc.lifecycle_record_op(
                TEST_TENANT, "scenario_run", _run(phase, "commit-a", "failed")
            )
        )
        passed = await decision.log(
            dc.lifecycle_record_op(
                TEST_TENANT, "scenario_run", _run(phase, "commit-b", "passed")
            )
        )
        repeat = await decision.log(
            dc.lifecycle_record_op(
                TEST_TENANT, "scenario_run", _run(phase, "commit-b", "passed")
            )
        )
        refusals = []
        for edit in ({"commit": ""}, {"phase": ""}):
            try:
                await decision.log(
                    dc.lifecycle_record_op(
                        TEST_TENANT,
                        "scenario_run",
                        _run(phase, "commit-c", "passed", **edit),
                    )
                )
            except Exception as error:
                refusals.append(str(error))
        found = await decision.log(
            dc.lifecycle_records_op(
                TEST_TENANT, "scenario_run", phase=phase, branch="main"
            )
        )
        return {
            "failed": failed,
            "passed": passed,
            "repeat": repeat,
            "refusals": refusals,
            "found": found,
        }
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R013
@pytest.mark.spec("MEDULLA-SDLC-R013")
def test_bundle_completes_when_rows_delivered_and_scenario_passed():
    journey = asyncio.run(_journey())
    assert not journey["passed"]["replayed"] and journey["repeat"]["replayed"]
    assert journey["repeat"]["record_id"] == journey["passed"]["record_id"]
    assert len(journey["refusals"]) == 2
    assert all("SCENARIO_RUN_INCOMPLETE" in text for text in journey["refusals"])
    runs = journey["found"]["scenario_runs"]
    assert [stored["run"]["commit"] for stored in runs] == ["commit-b", "commit-a"]
    # The latest run of the phase on main passed: the bundle rule's second half holds.
    assert runs[0]["run"]["result"] == "passed"
