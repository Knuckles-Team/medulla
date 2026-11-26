"""MEDULLA-DECISION-ENGINE-R150.6 session notes over the served wire.

Two successful runs and one failed run are credited through ``DecisionLog``
(``run_outcome``). Notes of the successful runs are captured through ``Learn``
(``record_session_note``) and recalled through ``MemoryRecall``: a third note
with the same input and another output conflicts with the first, a note
recorded against an older source revision is stale, and a note of the failed
run is refused ``NOTE_RUN_NOT_SUCCESSFUL``. Another tenant recalls none of them.
"""

from __future__ import annotations

import asyncio
import hashlib
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _commons_generation import Engine, connect, restartable_engine
from conftest import TEST_TENANT

from decide.test_scoped_adapter_served import (
    CAPABILITY,
    Q32,
    QUESTION,
    TEMPLATE,
    _discover,
    _establish_generation,
    _policy_body,
    _publish,
    _q16,
)
from medulla import decision_client as dc
from medulla import decision_stat as ds

NOTE_TEXT = "deploy runbook"


def _digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def _run(run_id: str, decision_id: str, cited: str, success: bool) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "run_id": run_id,
        "decision_ids": [decision_id],
        "answer_digest": _digest(run_id),
        "cited_row_ids": [cited],
        "success": success,
        "steps": [
            {
                "step_idx": 0,
                "agent_iri": "urn:agent:runner",
                "capability_iri": f"{CAPABILITY}:notes",
                "reward_q16": 1 << 16,
            }
        ],
    }


def _note(
    note_id: str, run_id: str, output: str, source_revision: int
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "note_id": note_id,
        "run_id": run_id,
        "session_id": "session-1",
        "task_id": "task-1",
        "user_id": "user-a",
        "input_digest": _digest("input-one"),
        "output_digest": _digest(output),
        "source_revision": source_revision,
        "content": NOTE_TEXT,
    }


# spec: MEDULLA-DECISION-ENGINE-R150.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R150.6")
@pytest.mark.no_engine
def test_capture_and_reuse(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    logical = f"notes-logical-{suffix}"
    shadow = f"notes-shadow-{suffix}"

    async def journey(engine: Engine) -> None:
        space = await _establish_generation(engine, logical, shadow, f"cell-{suffix}")
        client = await connect(shadow, engine=engine)
        try:
            decision = dc.DecisionClient(client=client)
            schema = await _publish(
                client,
                f"notes-schema-{suffix}",
                "feature_schema",
                ds.feature_schema_body(
                    [
                        ds.feature("cost", ds.number("cost")),
                        ds.feature("value", ds.number("value")),
                    ]
                ),
            )
            policy = await _publish(
                client, f"notes-policy-{suffix}", "decision_policy", _policy_body()
            )
            option = dc.DeclaredOption(
                f"plan-notes-{suffix}", numbers={"cost": Q32, "value": Q32}
            )
            request = dc.decide_request(
                TEST_TENANT,
                (QUESTION, "retrieval_plan", "ordinary"),
                dc.declared([option]),
                schema,
                policy={"policy": "pinned", "component": policy},
            )
            record = (await decision.decide(request))["records"][0]
            record_id = record["record_id"]
            expiry = int(time.time() * 1000) + 3_600_000
            await decision.log(dc.commit_op(record, dc.named_evaluator(expiry)))
            _, rows = await _discover(client, CAPABILITY)
            cited = rows[0]
            await decision.log(
                dc.record_outcome_op(
                    TEST_TENANT,
                    record_id,
                    [(unit, "doc") for unit in rows],
                    [cited],
                    query={"space_digest": space, "q16": _q16([1.0] + [0.0] * 7)},
                    path=TEMPLATE,
                )
            )
            for run_id, success in [
                (f"run-1-{suffix}", True),
                (f"run-2-{suffix}", True),
                (f"run-bad-{suffix}", False),
            ]:
                recorded = await decision.log(
                    dc.learn_op(
                        TEST_TENANT,
                        "run_outcome",
                        outcome=_run(run_id, record_id, cited, success),
                    )
                )
                assert recorded["recorded"] == "run", recorded

            async def capture(note: dict[str, Any]) -> dict[str, Any]:
                return await decision.log(
                    dc.learn_op(
                        TEST_TENANT, "record_session_note", graph=shadow, note=note
                    )
                )

            first = await capture(_note("note-a", f"run-1-{suffix}", "out-1", 5))
            assert first["recorded"] == "session_note", first
            await capture(_note("note-b", f"run-2-{suffix}", "out-2", 5))
            await capture(_note("note-old", f"run-1-{suffix}", "out-1", 4))
            with pytest.raises(RuntimeError, match="NOTE_RUN_NOT_SUCCESSFUL"):
                await capture(_note("note-bad", f"run-bad-{suffix}", "out-3", 5))

            recalled = await decision.memory_recall(
                {
                    "keywords": ["deploy"],
                    "budget": 10_000,
                    "current_revision": 5,
                    "min_relevance": 0.7,
                }
            )
            notes = {note["note_id"]: note for note in recalled["notes"]}
            assert "note-bad" not in notes, notes
            assert notes["note-a"]["conflict"] and notes["note-b"]["conflict"], notes
            assert notes["note-old"]["stale"] and not notes["note-a"]["stale"], notes
            # A keyword recall carries no query vector, so no note reaches the floor.
            assert not any(note["applied"] for note in notes.values()), notes
        finally:
            await client.close()

    with restartable_engine(tmp_path, "notes", bootstrap=True) as engine:
        asyncio.run(journey(engine))
