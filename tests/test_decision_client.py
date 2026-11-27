"""The typed decision client builds what the wire models accept (EH-065)."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla import decision_client as dc
from medulla.generated import models

pytestmark = pytest.mark.no_engine

PIN = {"component_id": "s", "kind": "feature_schema", "definition_digest": "sha256:x"}


def valid(model: Any, value: Any) -> bool:
    """Whether ``value`` validates as the wire model (extra fields forbidden)."""
    try:
        TypeAdapter(model).validate_python(value)
    except ValidationError:
        return False
    return True


def _options() -> list[dc.DeclaredOption]:
    return [
        dc.DeclaredOption("plan-hyde", {"threshold": dc.q32_of(0.38)}),
        dc.DeclaredOption(
            "plan-deep", {"threshold": dc.q32_of(0.28)}, {"label": "deep"}
        ),
    ]


def test_a_declared_decide_request_matches_the_contract() -> None:
    request = dc.decide_request(
        "tenant-a",
        ("au.retrieval.plan", "retrieval_plan", "ordinary"),
        dc.declared(_options()),
        PIN,
        params=[dc.param("query", "text", "who owns billing")],
    )
    assert valid(models.DecideRequest, request)
    assert [o["option_id"] for o in request["candidates"]["options"]] == [
        "plan-deep",
        "plan-hyde",
    ]
    broken = dict(request, candidates={"source": "declared", "options": [{"id": "x"}]})
    assert not valid(models.DecideRequest, broken), "the checker catches a bad shape"


def test_a_named_evaluator_is_a_principal_or_a_policy_role() -> None:
    by_role = dc.named_evaluator(1, role="decide-evaluator")
    by_principal = dc.named_evaluator(1, principal="principal:sha256:" + "0" * 64)
    for named in (by_role, by_principal):
        assert valid(models.NamedEvaluator, named), named


def test_every_log_op_matches_the_contract() -> None:
    ops = [
        dc.get_op("tenant-a", "decision:abc"),
        dc.resolve_op("tenant-a", "decision:abc", "r-1", "plan-deep"),
        dc.resolve_op("tenant-a", "decision:abc", "r-2", "plan-deep", producer="llm"),
        dc.aggregate_op("tenant-a", (0, 10), "au.retrieval.plan"),
        dc.record_outcome_op(
            "tenant-a", "decision:abc", [("n1", "prose"), ("p1", None)], ["p1"]
        ),
        dc.learn_op(
            "tenant-a",
            "move_pointer",
            pointer={"pointer": "adapter", "graph": "kg"},
            movement={"movement": "rollback"},
        ),
    ]
    for op in ops:
        assert valid(models.DecisionLogOp, op), op


def test_duplicate_declared_ids_are_refused_before_the_wire() -> None:
    with pytest.raises(ValueError):
        dc.declared([dc.DeclaredOption("a"), dc.DeclaredOption("a")])


def test_the_client_sends_through_the_generated_senders() -> None:
    sent: list[tuple[str, Any]] = []

    class _Client:
        async def _send(self, method: str, params: Any, graph: Any, **_: Any) -> Any:
            sent.append((method, params))
            return {"records": []}

    client = dc.DecisionClient(_Client(), graph="tenant-a")
    request = dc.decide_request(
        "tenant-a", ("q", "route", "ordinary"), dc.declared(_options()), PIN
    )
    assert asyncio.run(client.decide(request)) == {"records": []}
    asyncio.run(client.log(dc.get_op("tenant-a", "decision:abc")))
    assert [m for m, _ in sent] == ["Decide", "DecisionLog"]


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R076")
def test_decide_can_opt_into_the_nested_pydantic_decision_batch_model() -> None:
    """MEDULLA-DECISION-ENGINE-R076: as_model=True yields the nested DecisionBatch
    model; the default (as_model=False, exercised above) stays the raw
    payload, so other callers of this client are unaffected."""
    from medulla.generated.models import DecisionBatch

    class _Client:
        async def _send(self, method: str, params: Any, graph: Any, **_: Any) -> Any:
            return {
                "schema_version": 2,
                "inputs_digest": "sha256:" + "0" * 64,
                "records": [],
            }

    client = dc.DecisionClient(_Client(), graph="tenant-a")
    request = dc.decide_request(
        "tenant-a", ("q", "route", "ordinary"), dc.declared(_options()), PIN
    )
    model = asyncio.run(client.decide(request, as_model=True))
    assert isinstance(model, DecisionBatch)
    assert model.schema_version == 2
    assert model.records == []


# spec: MEDULLA-DECISION-ENGINE-R029.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R029.3")
def test_retrieval_plan_request_declares_registered_plans_and_pins_the_provider() -> (
    None
):
    provider = {
        "component_id": "mcp:docs/tool/search",
        "kind": "tool",
        "definition_digest": "sha256:" + "a" * 64,
    }
    request = dc.retrieval_plan_request(
        "tenant-a", "arbora.retrieval.space", PIN, provider=provider
    )
    assert valid(models.DecideRequest, request)
    assert request["question"]["kind"] == "retrieval_plan"
    assert [o["option_id"] for o in request["candidates"]["options"]] == sorted(
        dc.RETRIEVAL_PLAN_OPTIONS
    )
    assert [p["name"] for p in request["params"]] == ["retrieval.provider"]


# spec: MEDULLA-SDLC-R019.16
@pytest.mark.spec("MEDULLA-SDLC-R019.16")
def test_merge_arming_request_is_typed_and_carries_no_caller_receipt() -> None:
    arming = {
        "graph": "g",
        "delivery_id": "d1",
        "proposal_id": "p1",
        "head_commit": "sha256:" + "b" * 64,
        "head_changeset": "c1",
        "branch": "main",
        "required_check": "gate-1",
        "changed_lines": 12,
    }
    request = dc.merge_arming_request("tenant-a", "arbora.merge.arming", arming, PIN)
    assert valid(models.DecideRequest, request)
    assert request["question"]["kind"] == "merge_arming"
    names = [p["name"] for p in request["params"]]
    assert "changed_lines" in names and "approval" not in names


# spec: MEDULLA-DECISION-ENGINE-R829.4
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R829.4")
def test_catalog_rank_schemas_are_published_per_point_over_one_body() -> None:
    schemas = dc.CATALOG_RANK_FEATURE_SCHEMAS
    assert sorted(schemas) == sorted(
        [
            "catalog.skill_rank",
            "catalog.operation_rank",
            "catalog.catalog_item_rank",
            "catalog.intent_action_rank",
        ]
    )
    digests = {entry["content_digest"] for entry in schemas.values()}
    assert len(digests) == 1
    operation = schemas["catalog.operation_rank"]
    assert operation["component_id"] == "decide.schema.catalog.operation_rank"
    names = [f["name"] for f in operation["body"]["features"]]
    assert names[-1] == "semantic_similarity"
    assert operation["body"]["features"][-1]["missing"]["missing"] == "impute"


# spec: MEDULLA-DECISION-ENGINE-R180.8
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R180.8")
def test_process_control_schemas_are_published_per_step_over_one_body() -> None:
    schemas = dc.PROCESS_CONTROL_FEATURE_SCHEMAS
    assert sorted(schemas) == ["process.branch_select", "process.decision_table_eval"]
    assert len({entry["content_digest"] for entry in schemas.values()}) == 1
    branch = schemas["process.branch_select"]
    assert branch["component_id"] == "decide.schema.process.branch_select"
    assert branch["kind"] == "feature_schema"
    assert branch["content_digest"].startswith("sha256:")
    assert branch["idempotency_key"].startswith(
        "eg:decide.schema.process.branch_select:"
    )
    feature = branch["body"]["features"][0]
    assert feature["kind"] == {"feature": "declared_cost_micros"}
    assert feature["missing"]["missing"] == "impute"
