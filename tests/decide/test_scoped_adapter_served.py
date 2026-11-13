"""MEDULLA-DECISION-ENGINE-R174.7.6: an outcome-fitted capability adapter over the wire.

The entry point is the served ``DecisionLog`` method followed by served
``Discover``. The first generation of a graph is established through the served
generation cycle. Retrieval-plan records are decided (a cold-start policy
explores the one declared plan), committed with a named evaluator and attested
with the units they returned and cited (``learn`` op ``record_outcome``, with
the query vector and the plan template). A second principal judges each one.
``fit_adapter`` at capability scope, then ``move_pointer`` activate, re-orders
the ``Discover`` rows of that capability and names the adapter digest in the
verdict. Degrading outcomes and ``monitor_adapter`` roll it back to ``base``.
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _commons_generation import DIM, Engine, connect, restartable_engine, write_docs
from _decision_resident_fixture import publication
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.decision import policy_digest

MODEL = {"id": "adapter-model", "revision": "1", "dimension": DIM}
DOCS = 60
QUESTION = "q:retrieval-plan"
CAPABILITY = "urn:task:incident-triage"
JUDGE_ROLE = "outcome-judge"
FIRST_RUN = 40
DEGRADING_RUNS = 120
Q32 = 1 << 32
EVERY_TIME = {"from_ms": 0, "to_ms": (1 << 64) - 1}
SCOPE = {"scope": "capability", "capability_iri": CAPABILITY}
TEMPLATE = {
    "task_class": CAPABILITY,
    "composed_digest": "sha256:schema-a",
    "policy_version": "1",
    "anchor_class": "Incident",
    "edges": [{"relationship": "AFFECTS", "min_hops": 1, "max_hops": 2}],
    "rank": "vector",
    "slots": [],
    "skill_ref": "skill.incident-triage",
}


def _policy_body() -> dict[str, Any]:
    """The engine default policy, except that exploration is permitted for the
    retrieval-plan question: with one declared plan the explored option is it."""
    return {
        "schema_version": 1,
        "objective": {
            "order": "lexicographic",
            "levels": [
                "uncovered",
                "components",
                "declared_cost",
                "declared_p95_latency",
            ],
        },
        "unknown_cost": "exclude_when_strict",
        "accepted_gap": "0",
        "node_budget": 100000,
        "max_templates": 8,
        "max_slots": 6,
        "max_nogood_rounds": 8,
        "max_why_not_per_slot": 3,
        "a2a_requires_observation": True,
        "cold_start": {
            "mode": "explore",
            "budget": {
                "fraction": {"numerator": 1, "denominator": 1},
                "spend_at_risk_micros": 0,
                "questions": [QUESTION],
            },
        },
        "statistical": None,
    }


async def _publish(
    client: Any, component_id: str, kind: str, body: dict[str, Any]
) -> dict[str, Any]:
    request = publication(component_id, kind, body)
    if kind == "decision_policy":
        component = request["op"]["request"]["component"]
        component["content_digest"] = policy_digest(body)
        component["attributes"] = {"decision.policy": json.dumps(body)}
    entry = (await client._send("AgentComponent", request))["component"]
    return {
        "component_id": component_id,
        "kind": kind,
        "definition_digest": entry["definition_digest"],
    }


async def _establish_generation(
    engine: Engine, logical: str, shadow: str, cell: str
) -> str:
    """The first generation of ``logical``, served from ``shadow``: the model's
    space digest."""
    client = await connect(engine=engine)
    try:
        await client.tenants.create(logical)
        await client.tenants.create(shadow)
        now = int(time.time() * 1000)
        await client.capacity_leases.update_cell(
            {
                "schema_version": "1",
                "cell": {
                    "cell_id": cell,
                    "resource_class": "llm_embedding",
                    "capacity": 8,
                    "reserved_floor": 0,
                    "epoch": 1,
                    "policy_digest": "sha256:" + "d" * 64,
                    "updated_at_ms": now,
                },
                "expected_epoch": None,
                "now_ms": now,
            }
        )
        decision = dc.DecisionClient(client=client)
        read = await decision.log(
            {
                "op": "generation_space",
                "tenant_id": TEST_TENANT,
                "logical": logical,
                "model": MODEL,
            }
        )
        space = read["model_space_digest"]
    finally:
        await client.close()
    client = await connect(shadow, engine=engine)
    try:
        await write_docs(client, shadow, DOCS, f"r174-7-6:{shadow}")
        decision = dc.DecisionClient(client=client)
        done = await decision.log(
            dc.learn_op(
                TEST_TENANT,
                "run_generation_cycle",
                request={
                    "model_ref": MODEL,
                    "evaluation": {
                        "logical": logical,
                        "active_graph": logical,
                        "active_space": "none",
                        "shadow_graph": shadow,
                        "shadow_space": space,
                        "items": [],
                        "top_k": 5,
                        "min_eval_items": 5,
                        "max_score_psi_q16": 64 << 16,
                    },
                    "capacity_cell": cell,
                    "max_batches": None,
                },
            )
        )
        assert done["cycle"]["state"] == "activated", done
        return space
    finally:
        await client.close()


def _q16(vector: list[float]) -> list[int]:
    return [round(value * 65536) for value in vector]


async def _discover(
    client: Any, capability: str | None = None
) -> tuple[str, list[str]]:
    """The verdict's adapter digest and the hit ids in the order Discover ranks them."""
    found = await client.discover(
        [], [1.0] + [0.0] * (DIM - 1), k=5, capability=capability
    )
    return found["verdict"]["adapter_digest"], [hit["id"] for hit in found["hits"]]


async def _run(
    decision: dc.DecisionClient,
    judge: dc.DecisionClient,
    schema: dict[str, Any],
    policy: dict[str, Any],
    space: str,
    name: str,
    returned: list[str],
    cited: str,
    index: int,
) -> None:
    """One retrieval-plan run: decided, committed to the judge, attested, judged."""
    option = dc.DeclaredOption(
        f"plan-{name}-{index:03}", numbers={"cost": Q32, "value": Q32}
    )
    request = dc.decide_request(
        TEST_TENANT,
        (QUESTION, "retrieval_plan", "ordinary"),
        dc.declared([option]),
        schema,
        policy={"policy": "pinned", "component": policy},
    )
    record = (await decision.decide(request))["records"][0]
    assert record["outcome"]["outcome"] in ("explored", "acted"), record["outcome"]
    expiry = int(time.time() * 1000) + 3_600_000
    await decision.log(
        dc.commit_op(record, dc.named_evaluator(expiry, role=JUDGE_ROLE))
    )
    attested = await decision.log(
        dc.record_outcome_op(
            TEST_TENANT,
            record["record_id"],
            [(unit, "doc") for unit in returned],
            [cited],
            query={
                "space_digest": space,
                "q16": _q16([1.0, 0.0005 * (index % 40)] + [0.0] * (DIM - 2)),
            },
            path=TEMPLATE,
        )
    )
    assert attested["recorded"] == "outcome", attested
    judged = await judge.log(
        dc.evaluate_op(
            TEST_TENANT,
            {
                "record_id": record["record_id"],
                "evaluation_id": f"eval-{name}-{index:03}",
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


# spec: MEDULLA-DECISION-ENGINE-R174.7.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R174.7.6")
@pytest.mark.no_engine
def test_outcome_adapter_lifts_and_reverts(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    logical = f"adapter-logical-{suffix}"
    shadow = f"adapter-shadow-{suffix}"

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
                f"adapter-schema-{suffix}",
                "feature_schema",
                ds.feature_schema_body(
                    [
                        ds.feature("cost", ds.number("cost")),
                        ds.feature("value", ds.number("value")),
                    ]
                ),
            )
            policy = await _publish(
                client, f"adapter-policy-{suffix}", "decision_policy", _policy_body()
            )

            base_digest, base_rows = await _discover(client, CAPABILITY)
            assert base_digest == "base", base_digest
            assert len(base_rows) == 5, base_rows
            favoured = base_rows[3]
            incumbent = base_rows[0]
            assert favoured != incumbent

            # Cited outcomes: the answers cite the unit the base order buries.
            for index in range(FIRST_RUN):
                await _run(
                    decision,
                    judge,
                    schema,
                    policy,
                    space,
                    "rec",
                    base_rows,
                    favoured,
                    index,
                )

            fitted = await decision.log(
                dc.learn_op(
                    TEST_TENANT,
                    "fit_adapter",
                    request={
                        "graph": shadow,
                        "space_digest": space,
                        "question_id": None,
                        "window": EVERY_TIME,
                        "rank": 2,
                        "max_gain_q16": 1 << 15,
                        "holdout_per_mille": 300,
                        "min_eval_items": 5,
                        "scope": SCOPE,
                        "side": "query",
                        "sources": ["retrieval_outcome"],
                    },
                )
            )
            assert fitted["recorded"] == "fitted", fitted
            assert fitted["receipt"]["passed"], fitted["receipt"]
            assert fitted["receipt"]["scope"] == SCOPE, fitted["receipt"]
            pointer = {"pointer": "scoped_adapter", "graph": shadow, "scope": SCOPE}

            # Fitted but not active: the served order is still the base order.
            assert await _discover(client, CAPABILITY) == (base_digest, base_rows)

            moved = await decision.log(
                dc.learn_op(
                    TEST_TENANT,
                    "move_pointer",
                    pointer=pointer,
                    movement={
                        "movement": "activate",
                        "target": fitted["adapter_digest"],
                        "receipt_digest": fitted["receipt_digest"],
                    },
                )
            )
            assert moved["recorded"] == "pointer", moved

            # The capability's own read applies the adapter: its digest is named
            # and the cited unit now ranks ahead of where the base put it.
            lifted_digest, lifted_rows = await _discover(client, CAPABILITY)
            assert lifted_digest == fitted["adapter_digest"], lifted_digest
            assert lifted_rows != base_rows, lifted_rows
            assert lifted_rows.index(favoured) < base_rows.index(favoured), (
                base_rows,
                lifted_rows,
            )
            # A read naming no capability or another one stays on the base.
            assert await _discover(client) == (base_digest, base_rows)
            assert await _discover(client, "urn:task:other") == (
                base_digest,
                base_rows,
            )

            # Degrading outcomes: the answers now cite the base's first unit,
            # which the adapter ranks lower.
            for index in range(DEGRADING_RUNS):
                await _run(
                    decision,
                    judge,
                    schema,
                    policy,
                    space,
                    "deg",
                    lifted_rows,
                    incumbent,
                    index,
                )
            monitored = await decision.log(
                dc.learn_op(
                    TEST_TENANT,
                    "monitor_adapter",
                    request={"graph": shadow, "scope": SCOPE, "window": EVERY_TIME},
                )
            )
            assert monitored["recorded"] == "monitored", monitored
            assert monitored["state"] == "rolled_back", monitored

            # After the rollback Discover serves the base order again.
            assert await _discover(client, CAPABILITY) == (base_digest, base_rows)
        finally:
            await judge_client.close()
            await client.close()

    with restartable_engine(tmp_path, "adapter", bootstrap=True) as engine:
        asyncio.run(journey(engine))
