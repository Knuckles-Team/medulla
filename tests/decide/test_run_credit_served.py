"""MEDULLA-DECISION-ENGINE-R100.102.6: run credit over the served wire.

The entry point is the served ``DecisionLog`` method (``learn`` op
``run_outcome``) after served ``Discover``. A retrieval-plan record is decided,
committed with a named evaluator, attested with the units ``Discover`` returned
and judged by a second principal. One ``RunOutcome`` of 256 steps then credits
every step. Both reads use served surfaces only: the hard negatives are read from
the ``decision_hard_negatives`` relation through the generated SQL client, and
the moved baseline is observed through the run receipt, since the baseline has
no direct read.
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
from conftest import TEST_TENANT, request_context

from decide.test_scoped_adapter_served import (
    CAPABILITY,
    JUDGE_ROLE,
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

STEPS = 256


def _receipt(recorded: dict[str, Any]) -> dict[str, Any]:
    """The run credit receipt of a ``run`` learning result."""
    assert recorded["recorded"] == "run", recorded
    return recorded.get("receipt", recorded)


def _run_outcome(
    run_id: str, decision_id: str, cited: str, answer: str
) -> dict[str, Any]:
    """A successful 256-step run: one step per capability, each rewarded in full."""
    return {
        "schema_version": 1,
        "run_id": run_id,
        "decision_ids": [decision_id],
        "answer_digest": hashlib.sha256(answer.encode()).hexdigest(),
        "cited_row_ids": [cited],
        "success": True,
        "steps": [
            {
                "step_idx": index,
                "agent_iri": f"urn:agent:step-{index}",
                "capability_iri": f"{CAPABILITY}:step-{index}",
                "reward_q16": 1 << 16,
            }
            for index in range(STEPS)
        ],
    }


# spec: MEDULLA-DECISION-ENGINE-R100.102.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R100.102.6")
@pytest.mark.no_engine
def test_cited_rows_and_step_credit_recorded(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    logical = f"credit-logical-{suffix}"
    shadow = f"credit-shadow-{suffix}"

    async def journey(engine: Engine) -> None:
        space = await _establish_generation(engine, logical, shadow, f"cell-{suffix}")
        client = await connect(shadow, engine=engine)
        judge_client = await connect(
            shadow,
            engine=engine,
            context=request_context(
                principal="outcome-judge-principal",
                roles=["test", JUDGE_ROLE],
            ),
        )
        try:
            decision = dc.DecisionClient(client=client)
            judge = dc.DecisionClient(client=judge_client)
            schema = await _publish(
                client,
                f"credit-schema-{suffix}",
                "feature_schema",
                ds.feature_schema_body(
                    [
                        ds.feature("cost", ds.number("cost")),
                        ds.feature("value", ds.number("value")),
                    ]
                ),
            )
            policy = await _publish(
                client, f"credit-policy-{suffix}", "decision_policy", _policy_body()
            )

            # A retrieval-plan record, decided and committed to the judge.
            option = dc.DeclaredOption(
                f"plan-credit-{suffix}", numbers={"cost": Q32, "value": Q32}
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
            await decision.log(
                dc.commit_op(record, dc.named_evaluator(expiry, role=JUDGE_ROLE))
            )

            # The run's rows come from the served Discover; the answer cites the
            # unit the order buries, so the units ranked above it are hard negatives.
            _, rows = await _discover(client, CAPABILITY)
            assert len(rows) == 5, rows
            cited = rows[3]
            attested = await decision.log(
                dc.record_outcome_op(
                    TEST_TENANT,
                    record_id,
                    [(unit, "doc") for unit in rows],
                    [cited],
                    query={
                        "space_digest": space,
                        "q16": _q16([1.0] + [0.0] * 7),
                    },
                    path=TEMPLATE,
                )
            )
            assert attested["recorded"] == "outcome", attested

            first = _receipt(
                await decision.log(
                    dc.learn_op(
                        TEST_TENANT,
                        "run_outcome",
                        outcome=_run_outcome(f"run-1-{suffix}", record_id, cited, "a"),
                    )
                )
            )
            assert first["run_id"] == f"run-1-{suffix}", first
            assert first["positive_count"] == 1 and first["negative_count"] == 3, first
            assert len(first["steps"]) == STEPS, len(first["steps"])
            assert {step["baseline_revision"] for step in first["steps"]} == {0}

            # Hard negatives list only runs judged Success by a named evaluator, so
            # the view is empty until the judge principal judges the run.
            columns = "record_id, evidence_id, rank, outranked_cited"
            before = await client.query.sql(
                f"SELECT {columns} FROM decision_hard_negatives"
            )
            assert before == [], before
            judged = await judge.log(
                dc.evaluate_op(
                    TEST_TENANT,
                    {
                        "record_id": record_id,
                        "evaluation_id": f"eval-credit-{suffix}",
                        "class": "observation",
                        "selected_agent": "retriever-agent",
                        "lease_holder": "retriever-worker",
                        "fidelity": "tool_calls",
                        "success": True,
                        "backbone": None,
                    },
                )
            )
            assert judged, judged
            negatives = await client.query.sql(
                f"SELECT {columns} FROM decision_hard_negatives"
            )
            assert sorted(row["evidence_id"] for row in negatives) == sorted(
                rows[:3]
            ), negatives
            assert {row["record_id"] for row in negatives} == {record_id}, negatives

            # The baseline moved: the second run measures every step against the
            # revision the first run wrote.
            second = _receipt(
                await decision.log(
                    dc.learn_op(
                        TEST_TENANT,
                        "run_outcome",
                        outcome=_run_outcome(f"run-2-{suffix}", record_id, cited, "b"),
                    )
                )
            )
            assert len(second["steps"]) == STEPS, len(second["steps"])
            for before_step, after_step in zip(
                first["steps"], second["steps"], strict=True
            ):
                assert before_step["step_idx"] == after_step["step_idx"]
                assert (
                    after_step["baseline_revision"] > before_step["baseline_revision"]
                )
        finally:
            await judge_client.close()
            await client.close()

    with restartable_engine(tmp_path, "credit", bootstrap=True) as engine:
        asyncio.run(journey(engine))
