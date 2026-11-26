"""Typed client for the statistical decision surface (EH-065).

``medulla.decision_stat`` carries the body codec and feature builders;
this module is the request side: typed builders for ``Decide`` requests
(including ``CandidateSource::Declared`` options) and every ``DecisionLog`` op,
and :class:`DecisionClient`, which sends them through the generated contract
senders (``send_decide``, ``send_decision_log``) so the generated request
validation runs on every call.

Every dict lists its keys in Rust field order and uses the serde wire names
(snake_case enums, ``tag`` fields), so what is built here is exactly what the
engine decodes. Numbers on declared options are ``Q32`` integers -- never a
float on the wire.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from pydantic import BaseModel

from medulla.generated._task_plan_feature_schema import (
    TASK_PLAN_FEATURE_SCHEMA,
)

Q32_ONE = 1 << 32


def q32_of(value: float) -> int:
    """``value`` on the ``Q32`` scale (rounded to the nearest representable)."""
    return round(float(value) * Q32_ONE)


@dataclass(frozen=True, slots=True)
class DeclaredOption:
    """One caller-declared option: every fact is the caller's claim."""

    option_id: str
    numbers: Mapping[str, int] = field(default_factory=dict)
    texts: Mapping[str, str] = field(default_factory=dict)
    classification: tuple[str, ...] = ()

    def wire(self) -> dict[str, Any]:
        return {
            "option_id": self.option_id,
            "classification": list(self.classification),
            "numbers": [
                {"key": k, "q32": int(self.numbers[k])} for k in sorted(self.numbers)
            ],
            "texts": [{"key": k, "text": self.texts[k]} for k in sorted(self.texts)],
        }


def declared(options: Iterable[DeclaredOption]) -> dict[str, Any]:
    """``CandidateSource::Declared``; options sorted by id (the matrix order)."""
    ordered = sorted(options, key=lambda o: o.option_id)
    ids = [o.option_id for o in ordered]
    if len(set(ids)) != len(ids):
        raise ValueError("declared option ids must be unique")
    return {"source": "declared", "options": [o.wire() for o in ordered]}


def rank_catalog(options: Iterable[DeclaredOption]) -> dict[str, Any]:
    """``CandidateSource::RankCatalog``; options sorted by id, ids unique.

    The whole candidate set of a catalog goes in unfiltered (up to the engine's
    cap); the caller's own scores are not accepted, so options carry texts only.
    """
    ordered = sorted(options, key=lambda o: o.option_id)
    ids = [o.option_id for o in ordered]
    if len(set(ids)) != len(ids):
        raise ValueError("rank catalog option ids must be unique")
    return {"source": "rank_catalog", "options": [o.wire() for o in ordered]}


def library(
    kinds: Sequence[str], classification_under: str | None = None
) -> dict[str, Any]:
    """``CandidateSource::AgentLibrary`` over component kinds (wire names)."""
    return {
        "source": "agent_library",
        "scope": {"kinds": list(kinds), "classification_under": classification_under},
    }


def param(name: str, kind: str, value: Any) -> dict[str, Any]:
    """One ``TypedParam``, including the typed ``task_plan`` proposal variant."""
    return {"name": name, "value": {"type": kind, "value": value}}


def decide_request(
    tenant_id: str,
    question: tuple[str, str, str],
    candidates: Mapping[str, Any],
    feature_schema: Mapping[str, Any],
    *,
    head: Mapping[str, Any] | None = None,
    policy: Mapping[str, Any] | None = None,
    params: Iterable[Mapping[str, Any]] = (),
    max_records: int | None = None,
) -> dict[str, Any]:
    """A ``DecideRequest``; ``question`` is ``(question_id, kind, safety)``."""
    question_id, kind, safety = question
    return {
        "tenant_id": tenant_id,
        "question": {"question_id": question_id, "kind": kind, "safety": safety},
        "candidates": dict(candidates),
        "feature_schema": dict(feature_schema),
        "head": None if head is None else dict(head),
        "policy": dict(policy or {"policy": "default"}),
        "params": sorted((dict(p) for p in params), key=lambda p: str(p["name"])),
        "max_records": max_records,
    }


def capability_designation_request(
    tenant_id: str,
    question_id: str,
    question: Mapping[str, Any],
    graph: str,
    feature_schema: Mapping[str, Any],
    *,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """A ``capability_designation`` ``Decide`` request.

    ``question`` is the typed designation question (tenant, query vector,
    required capabilities and policy tags, ``k``, weights and ontology prior);
    ``graph`` names the graph whose composed schema supplies the capability
    class closure. The engine ranks the resident index: it never takes the
    candidates from the caller, so the one declared candidate is the question id.
    """
    return decide_request(
        tenant_id,
        (question_id, "capability_designation", "ordinary"),
        declared([DeclaredOption(question_id)]),
        feature_schema,
        policy=policy,
        params=[
            param(
                "capability_designation",
                "capability_designation",
                {"question": dict(question), "graph": graph},
            )
        ],
    )


def load_set_request(
    tenant_id: str,
    question_id: str,
    question: Mapping[str, Any],
    feature_schema: Mapping[str, Any],
    *,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """A ``load_set`` ``Decide`` request: which tools, skills and agents a job
    should have loaded.

    ``question`` is the typed load-set question (``job_text``, ``job_embedding``
    and its ``embedding_version``, ``required_capabilities``, ``max_tools``,
    ``max_agents`` and the ``graph`` whose schema holds the capability classes).
    The engine ranks the resident capability index and never takes candidates
    from the caller, so the one declared candidate is the question id.
    """
    return decide_request(
        tenant_id,
        (question_id, "load_set", "ordinary"),
        declared([DeclaredOption(question_id)]),
        feature_schema,
        policy=policy,
        params=[param("load_set", "load_set", dict(question))],
    )


def usage_guidance_request(
    tenant_id: str,
    question_id: str,
    connector: str,
    job_class: str,
    feature_schema: Mapping[str, Any],
    *,
    graph: str | None = None,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """A ``usage_guidance`` ``Decide`` request: how and when to use a connector.

    ``connector`` is the pack's connector node (``mcp-server://<name>``) and
    ``job_class`` the class whose ``:hasStep`` chain is ordered. The engine
    reads the pack and the tenant's tool outcomes itself, so the one declared
    candidate is the question id.
    """
    params = [
        param("connector", "iri", connector),
        param("job_class", "iri", job_class),
    ]
    if graph is not None:
        params.append(param("graph", "text", graph))
    return decide_request(
        tenant_id,
        (question_id, "usage_guidance", "ordinary"),
        declared([DeclaredOption(question_id)]),
        feature_schema,
        policy=policy,
        params=params,
    )


def task_plan_request(
    tenant_id: str,
    question_id: str,
    proposal: Mapping[str, Any],
    feature_schema: Mapping[str, Any],
    *,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Build an evaluate-only explicit plan request, never an admission grant.

    The caller supplies host-selected limits and a real published schema pin.
    Model proposals must not provide the tenant, pin, policy or limits. Graph
    validity and effective bounds are computed by the engine, not this helper.
    """
    import json

    from medulla.generated.models import TaskPlanProposal

    # Strict JSON mode admits the wire strings of Rust enums (``"parallel"``,
    # ``"eq"``) while refusing coercions such as a quoted integer budget.
    typed = TaskPlanProposal.model_validate_json(
        json.dumps(dict(proposal)), strict=True
    )
    # ``TaskPlanTask::call`` carries ``skip_serializing_if = "Option::is_none"``
    # on the engine side, so an absent call is an absent key -- never ``null``.
    # The generated senders dump the same way (``exclude_none=True``).
    # ``by_alias`` keeps a foreach source's wire key ``$ref``.
    body = typed.model_dump(mode="json", exclude_none=True, by_alias=True)
    return decide_request(
        tenant_id,
        (question_id, "task_plan", "ordinary"),
        declared(DeclaredOption(task["task_id"]) for task in body["tasks"]),
        feature_schema,
        policy=policy,
        params=[param("task_plan", "task_plan", body)],
        max_records=1,
    )


def skill_plan_request(
    tenant_id: str,
    question_id: str,
    query: Mapping[str, Any],
    feature_schema: Mapping[str, Any],
    *,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Build an automatic ``TaskPlan`` question over the tenant's published skills.

    ``query`` is the ``SkillPlanQuery`` wire shape: ``output_task_id`` and
    ``subtasks`` (``id``, ``text``, ``depends_on``). The engine covers each
    subtask with a published skill and returns the dependency-ordered plan.
    """
    return decide_request(
        tenant_id,
        (question_id, "task_plan", "ordinary"),
        declared(DeclaredOption(task["id"]) for task in query["subtasks"]),
        feature_schema,
        policy=policy,
        params=[param("skill_query", "skill_plan_query", query)],
        max_records=1,
    )


def tool_assignment_request(
    tenant_id: str,
    question_id: str,
    feature_schema: Mapping[str, Any],
    *,
    task_text: str,
    agent_role_iri: str = "",
    top_k: int = 10,
    context_budget_tokens: int = 1000,
    required_capabilities: Sequence[str] = (),
    task_embedding: Sequence[int] | None = None,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Build a ``ToolRelevance`` question over the tenant's published tools.

    The typed ``tool_assignment`` parameter carries the task, the role (empty
    for the whole catalog), the ``top_k`` and the token budget; the engine
    ranks the published ``Tool`` components through its tool index. The
    optional embedding is ``Q32`` integers, never floats.
    """
    body = {
        "schema_version": 1,
        "task_text": task_text,
        "task_embedding": None if task_embedding is None else list(task_embedding),
        "agent_role_iri": agent_role_iri,
        "top_k": top_k,
        "context_budget_tokens": context_budget_tokens,
        "required_capabilities": list(required_capabilities),
    }
    return decide_request(
        tenant_id,
        (question_id, "tool_relevance", "ordinary"),
        library(["tool"]),
        feature_schema,
        policy=policy,
        params=[param("tool_assignment", "tool_assignment", body)],
    )


def process_plan_request(
    tenant_id: str,
    question_id: str,
    feature_schema: Mapping[str, Any],
    *,
    graph: str,
    workflow_id: str,
    version: int,
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Build a ``ProcessPlan`` question over one stored canonical workflow
    (MEDULLA-CQ-ORCH-R704.5).

    ``graph`` names the graph that holds the workflow; ``workflow_id`` and
    ``version`` name the stored version (the ``workflow_id`` the import was
    given, not the node id it answered). The engine binds each step from the
    graph, then by tool relevance, and answers the sealed task plan in
    ``answer.value`` with one ``binding:`` premise per step. Nothing is written.
    """
    body = json.dumps(
        {"workflow_id": workflow_id, "version": version},
        sort_keys=True,
        separators=(",", ":"),
    )
    return decide_request(
        tenant_id,
        (question_id, "process_plan", "ordinary"),
        declared([DeclaredOption("process_plan")]),
        feature_schema,
        policy=policy,
        params=[param("graph", "text", graph), param("process_plan", "text", body)],
    )


def _json_value(value: Any) -> Any:
    """A ``serde_json::Value`` as the engine encodes it: object keys sorted."""
    if isinstance(value, Mapping):
        return {key: _json_value(value[key]) for key in sorted(value)}
    if isinstance(value, list):
        return [_json_value(item) for item in value]
    return value


def _task_plan_task(task: Mapping[str, Any]) -> dict[str, Any]:
    """One task in Rust declaration order, for the digest.

    ``call`` is present only when the caller declared it: the engine skips a
    ``None`` call when it serializes, and the digest covers those exact bytes.
    """
    body: dict[str, Any] = {
        "task_id": task["task_id"],
        "prompt": task["prompt"],
        "depends_on": task["depends_on"],
    }
    call = task.get("call")
    if call is not None:
        body["call"] = {
            "server": call["server"],
            "tool": call["tool"],
            "arguments": _json_value(call["arguments"]),
        }
    aggregation = task.get("aggregation")
    if aggregation is not None:
        body["aggregation"] = {"op": aggregation["op"]}
        if aggregation.get("reducer_id") is not None:
            body["aggregation"]["reducer_id"] = aggregation["reducer_id"]
    if task.get("assignee") is not None:
        body["assignee"] = task["assignee"]
    foreach = task.get("foreach")
    if foreach is not None:
        # Rust declaration order: ``source`` then ``max_items``.
        body["foreach"] = {
            "source": {"$ref": foreach["source"]["$ref"]},
            "max_items": foreach["max_items"],
        }
    _add_control_steps(body, task)
    for key in ("duration_ms", "priority_weight"):
        if task.get(key) is not None:
            body[key] = task[key]
    return body


def _condition_body(condition: Mapping[str, Any]) -> dict[str, Any]:
    if condition.get("decision_iri") is not None:
        return {"decision_iri": condition["decision_iri"]}
    return {
        "ref": condition["ref"],
        "op": condition["op"],
        "value": _json_value(condition["value"]),
    }


def _add_control_steps(body: dict[str, Any], task: Mapping[str, Any]) -> None:
    """Branch, join, decision and approval steps in Rust declaration order
    (MEDULLA-DECISION-ENGINE-R134.111.17); an unset step is an absent key."""
    branch = task.get("branch")
    if branch is not None:
        arms = []
        for arm in branch["arms"]:
            item: dict[str, Any] = {"target_task_id": arm["target_task_id"]}
            if arm.get("condition") is not None:
                item["condition"] = _condition_body(arm["condition"])
            arms.append(item)
        body["branch"] = {"kind": branch["kind"], "arms": arms}
        if branch.get("default_arm") is not None:
            body["branch"]["default_arm"] = branch["default_arm"]
        if branch.get("exhaustive"):
            body["branch"]["exhaustive"] = True
    join = task.get("join")
    if join is not None:
        body["join"] = {"branch_task_id": join["branch_task_id"]}
    decision = task.get("decision")
    if decision is not None:
        body["decision"] = {
            "decision_iri": decision["decision_iri"],
            "inputs": list(decision.get("inputs") or []),
        }
    approval = task.get("approval")
    if approval is not None:
        body["approval"] = {
            "approver_role": approval["approver_role"],
            "timeout_s": approval["timeout_s"],
        }


def _final_artifact(value: Mapping[str, Any]) -> dict[str, Any] | None:
    decl = value.get("final_artifact")
    if decl is None:
        return None
    return {
        "schema_id": decl["schema_id"],
        "schema_version": decl["schema_version"],
        "kind": decl["kind"],
    }


def _task_plan_body(value: Mapping[str, Any]) -> dict[str, Any]:
    """A validated proposal in Rust declaration order, for its digest."""
    body: dict[str, Any] = {
        "tasks": [_task_plan_task(task) for task in value["tasks"]],
        "output_task_id": value["output_task_id"],
        "max_concurrency": value["max_concurrency"],
        "max_steps": value["max_steps"],
    }
    final_artifact = _final_artifact(value)
    if final_artifact is not None:
        body["final_artifact"] = final_artifact
    return body


def _critical_chain(value: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if value is None:
        return None
    return {
        key: value[key]
        for key in (
            "critical_chain_step_ids",
            "total_weight_units",
            "total_weighted_duration_ms",
            "weight_scale",
            "policy_revision",
            "plan_digest",
            "input_digest",
        )
    }


def _horizons(value: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if value is None:
        return None
    return {
        "plan_digest": value["plan_digest"],
        "horizons": [
            {"index": horizon["index"], "step_ids": horizon["step_ids"]}
            for horizon in value["horizons"]
        ],
        "credit_ceiling": value["credit_ceiling"],
    }


def _plan_digest(body: Mapping[str, Any], plan: Mapping[str, Any]) -> str:
    """``compute_plan_digest``: v1 bare, v2 with the derived chain and
    horizons, v3 with those and the final-artifact declaration."""
    from medulla.decision._digest import digest_text

    core = [
        body["tasks"],
        body["output_task_id"],
        body["max_concurrency"],
        body["max_steps"],
        plan["total_tasks"],
        plan["policy_revision"],
        plan["policy_digest"],
    ]
    chain = _critical_chain(plan.get("critical_chain"))
    horizons = _horizons(plan.get("horizons"))
    final_artifact = body.get("final_artifact")
    if final_artifact is not None:
        return digest_text("eg/task-plan/v3", [core, chain, horizons, final_artifact])
    if chain is None and horizons is None:
        return digest_text("eg/task-plan/v1", core)
    return digest_text("eg/task-plan/v2", [core, chain, horizons])


def _require_task_plan(condition: bool, detail: str) -> None:
    if not condition:
        raise ValueError("task_plan receipt mismatch: " + detail)


def _verify_task_plan_question(
    asked: Mapping[str, Any], expected_principal: str
) -> None:
    _require_task_plan(
        asked["question"]["kind"] == "task_plan"
        and asked["question"]["safety"] == "ordinary"
        and asked["head"] is None
        and not asked["belief_as_of"],
        "question",
    )
    _require_task_plan(
        len(asked["params"]) == 1
        and asked["params"][0]["name"] == "task_plan"
        and asked["params"][0]["value"]["type"] == "task_plan",
        "proposal parameter",
    )
    _require_task_plan(
        isinstance(expected_principal, str) and bool(expected_principal.strip()),
        "expected caller principal",
    )


def _verify_task_plan_identity(
    received: Mapping[str, Any], asked: Mapping[str, Any], expected_principal: str
) -> None:
    import hashlib

    # Same byte-for-byte binding as VerifiedRequestContext::principal_persistence_id;
    # this does not turn AnonymousLocal into authenticated provenance.
    expected_caller = (
        "principal:sha256:"
        + hashlib.sha256(expected_principal.encode("utf-8")).hexdigest()
    )
    _require_task_plan(
        received["caller_principal"] == expected_caller, "caller principal"
    )
    _require_task_plan(
        received["schema_version"] == 2
        and received["tenant_id"] == asked["tenant_id"]
        and received["question"] == asked["question"]
        and bool(received["caller_principal"])
        and bool(received["record_id"])
        and bool(received["record_digest"]),
        "identity",
    )
    _require_task_plan(
        received["evidence_class"] == "claim"
        and received["resolution_kind"] == "constraint"
        and received["outcome"]
        == {
            "outcome": "advisory",
            "scores": [],
            "calibrated": False,
            "ranked_total": 0,
        },
        "evaluate-only evidence",
    )
    for key in ("params", "feature_schema", "head", "policy"):
        _require_task_plan(received["inputs"][key] == asked[key], key)


def _verify_task_plan_claims(
    plan: dict[str, Any], asked: Mapping[str, Any]
) -> dict[str, Any]:
    from medulla.decision._digest import digest_text

    proposal = _task_plan_body(asked["params"][0]["value"]["value"])
    body = _task_plan_body(plan)
    for key in ("output_task_id", "max_concurrency", "max_steps"):
        _require_task_plan(body[key] == proposal[key], key)
    expected_tasks = [
        {**task, "depends_on": sorted(task["depends_on"])} for task in proposal["tasks"]
    ]
    _require_task_plan(
        sorted(body["tasks"], key=lambda task: task["task_id"])
        == sorted(expected_tasks, key=lambda task: task["task_id"])
        and plan["total_tasks"] == len(expected_tasks),
        "task claims",
    )
    _require_task_plan(
        bool(plan["policy_revision"]) and bool(plan["policy_digest"]), "policy"
    )
    _require_task_plan(
        plan["input_digest"] == digest_text("eg/task-plan-input/v1", proposal),
        "input digest",
    )
    _require_task_plan(plan["plan_digest"] == _plan_digest(body, plan), "plan digest")
    return plan


def verify_task_plan_receipt(
    record: Mapping[str, Any], request: Mapping[str, Any], *, expected_principal: str
) -> dict[str, Any]:
    """Check a direct engine response's plan/request bindings; return its plan.

    ``expected_principal`` is the raw host caller principal. EG stores its
    domain-prefixed SHA-256 identifier, including for anonymous/local callers.
    This checks typed content and input/plan digests, not the full statistical
    record digest or transport authentication. A self-consistent caller receipt
    is NOT trustworthy. Use only a direct response from the configured engine.
    The host still owns authorization, policy acceptance and durable admission.
    No plan is persisted or admitted by this function.
    """
    from medulla.decision_stat import canonical_body_bytes
    from medulla.generated.models import (
        DecideRequest,
        StatisticalDecisionRecord,
    )

    # Strict JSON mode admits the wire strings of Rust enums, while refusing
    # coercions such as a quoted integer or boolean budget.
    asked = DecideRequest.model_validate_json(
        canonical_body_bytes(dict(request)), strict=True
    ).model_dump(mode="json", by_alias=True)
    decoded = StatisticalDecisionRecord.model_validate_json(
        canonical_body_bytes(dict(record)), strict=True
    )
    received = decoded.model_dump(mode="json", by_alias=True)

    _verify_task_plan_question(asked, expected_principal)
    _verify_task_plan_identity(received, asked, expected_principal)
    # Return the plan exactly as the engine serves it: a ``None`` task call is
    # an absent key (``skip_serializing_if``), not ``null``, and the caller
    # round-trips this body into ``PublishCurrentPlan`` unchanged.
    answer = decoded.answer
    served_plan = answer.value if answer is not None else None
    _require_task_plan(served_plan is not None, "missing plan")
    # _require_task_plan already raised for an absent plan; this branch only
    # narrows the type for the checker.
    if not isinstance(served_plan, BaseModel):
        raise ValueError("task_plan receipt mismatch: missing plan")
    plan = served_plan.model_dump(mode="json", by_alias=True, exclude_none=True)
    # ``BranchSpec::exhaustive`` is skipped on the wire when false.
    for task in plan["tasks"]:
        branch = task.get("branch")
        if branch is not None and branch.get("exhaustive") is False:
            del branch["exhaustive"]
    return _verify_task_plan_claims(plan, asked)


def named_evaluator(
    expires_at_ms: int, *, principal: str | None = None, role: str | None = None
) -> dict[str, Any]:
    """Who may evaluate a committed record until ``expires_at_ms`` (EH-395):
    one principal (its persistence id, never the committer) or the holders of
    one declared policy role -- exactly one of the two."""
    return {"principal": principal, "role": role, "expires_at_ms": expires_at_ms}


def commit_op(
    record: Mapping[str, Any], evaluator: Mapping[str, Any] | None = None
) -> dict[str, Any]:
    """Commit one record, optionally naming its evaluator (:func:`named_evaluator`)."""
    return {"op": "commit", "record": dict(record), "evaluator": evaluator}


def get_op(tenant_id: str, record_id: str) -> dict[str, Any]:
    return {"op": "get", "tenant_id": tenant_id, "record_id": record_id}


def resolve_op(
    tenant_id: str,
    record_id: str,
    resolution_id: str,
    option_id: str,
    *,
    producer: str | None = None,
    prompt_digest: str | None = None,
) -> dict[str, Any]:
    """Resolve a logged abstention: by the verified human caller when
    ``producer`` is ``None`` (an observation), else by that model (a claim)."""
    resolver: dict[str, Any] = (
        {"resolver": "human"}
        if producer is None
        else {"resolver": "model", "producer": producer, "prompt_digest": prompt_digest}
    )
    return {
        "op": "resolve",
        "tenant_id": tenant_id,
        "resolution": {
            "record_id": record_id,
            "resolution_id": resolution_id,
            "option_id": option_id,
            "resolver": resolver,
        },
    }


def promote_rule_op(
    tenant_id: str, rule: Mapping[str, Any], resolution_ids: Sequence[str]
) -> dict[str, Any]:
    """Promote a guardrail rule backed by stored abstention resolutions."""
    return {
        "op": "promote_rule",
        "tenant_id": tenant_id,
        "rule": dict(rule),
        "resolution_ids": list(resolution_ids),
    }


def propose_rule_op(
    tenant_id: str,
    rule: Mapping[str, Any],
    scope: str,
    source_output_digest: str,
    *,
    predecessor: int | None = None,
) -> dict[str, Any]:
    """Propose a new version of a rule, bound to the run output it corrects."""
    return {
        "op": "propose_rule",
        "tenant_id": tenant_id,
        "rule": dict(rule),
        "scope": scope,
        "source_output_digest": source_output_digest,
        "predecessor": predecessor,
    }


def approve_rule_op(
    tenant_id: str, rule_id: str, version: int, source_output_digest: str
) -> dict[str, Any]:
    """One approval of a proposed version; the second distinct approver activates it."""
    return {
        "op": "approve_rule",
        "tenant_id": tenant_id,
        "rule_id": rule_id,
        "version": version,
        "source_output_digest": source_output_digest,
    }


def revoke_rule_op(tenant_id: str, rule_id: str, version: int) -> dict[str, Any]:
    """Revoke the active version of a rule, keeping its history."""
    return {
        "op": "revoke_rule",
        "tenant_id": tenant_id,
        "rule_id": rule_id,
        "version": version,
    }


def policy_activation_op(
    tenant_id: str, change: str, source: str, receipt_digest: str
) -> dict[str, Any]:
    """Activate (``create``), replace (``update``) or withdraw (``retire``) the policy
    of the repository ``source`` behind the recorded review ``receipt_digest``."""
    return {
        "op": "policy_activation",
        "tenant_id": tenant_id,
        "change": change,
        "source": source,
        "receipt_digest": receipt_digest,
    }


def evaluate_op(tenant_id: str, evaluation: Mapping[str, Any]) -> dict[str, Any]:
    return {"op": "evaluate", "tenant_id": tenant_id, "evaluation": dict(evaluation)}


def memory_rewrite_op(proposal: Mapping[str, Any]) -> dict[str, Any]:
    """Evaluate one memory rewrite (R141.100) as a ``DecisionEval`` job."""
    return {"op": "memory_rewrite_submit", "request": dict(proposal)}


def memory_benchmark_op(request: Mapping[str, Any]) -> dict[str, Any]:
    """Score one memory change against a benchmark dataset (R141.101) as a
    ``DecisionEval`` job."""
    return {"op": "memory_change_benchmark_submit", "request": dict(request)}


def certify_local_skill_op(request: Mapping[str, Any]) -> dict[str, Any]:
    """Attest one local-model skill certification (R142.101) as a ``DecisionEval``
    job."""
    return {"op": "certify_local_skill", "request": dict(request)}


def delegation_quality_op(request: Mapping[str, Any]) -> dict[str, Any]:
    """Judge one window of a delegate's observations (R157.3) as a
    ``DecisionEval`` job."""
    return {"op": "delegation_quality", "request": dict(request)}


def skill_eval_op(request: Mapping[str, Any]) -> dict[str, Any]:
    """Evaluate one skill's suite of cases and store the results and the report
    (R711.3) as a ``DecisionEval`` job."""
    return {"op": "skill_eval", "request": dict(request)}


def aggregate_op(
    tenant_id: str, window: tuple[int, int], question_id: str | None = None
) -> dict[str, Any]:
    return {
        "op": "aggregate",
        "request": {
            "tenant_id": tenant_id,
            "question_id": question_id,
            "window": {"from_ms": window[0], "to_ms": window[1]},
        },
    }


def harness_adoption_op(tenant_id: str, **write: Any) -> dict[str, Any]:
    """Store one harness adoption with its contrast and lesson (R803.8).

    The write names the committed ``harness_contrast`` decision by its record
    digest; it never carries the contrast deltas, which the engine reads from
    that decision."""
    return {"op": "harness_adoption", "tenant_id": tenant_id, "write": dict(write)}


def harness_lessons_op(
    tenant_id: str,
    task_class: str,
    deficiency: str,
    as_of_ms: int,
    trajectory_features: list[str] | None = None,
) -> dict[str, Any]:
    """Read the stored harness lessons that apply, current as of ``as_of_ms``."""
    return {
        "op": "harness_lessons",
        "tenant_id": tenant_id,
        "query": {
            "task_class": task_class,
            "deficiency": deficiency,
            "trajectory_features": list(trajectory_features or []),
            "as_of_ms": as_of_ms,
        },
    }


def pointer_op(tenant_id: str, pointer_key: str) -> dict[str, Any]:
    """Read one champion pointer (R805.13): ``pointer_key`` is
    ``champion:<kind>:<ref>``. An unknown key refuses ``POINTER_NOT_FOUND``."""
    return {"op": "pointer", "tenant_id": tenant_id, "pointer_key": pointer_key}


def lifecycle_record_op(
    tenant_id: str, record_kind: str, record: dict[str, Any]
) -> dict[str, Any]:
    """Commit one lifecycle record (SDLC R013, R019.4): a ``scenario_run`` of a delivery
    phase or a ``would_dispatch`` decision. A repeat of an identical record replays."""
    return {
        "op": "lifecycle_record",
        "tenant_id": tenant_id,
        "record": {"record_kind": record_kind, "record": dict(record)},
    }


def lifecycle_records_op(
    tenant_id: str,
    record_kind: str,
    phase: str | None = None,
    branch: str | None = None,
) -> dict[str, Any]:
    """List the stored lifecycle records of one kind, newest first (SDLC R013)."""
    return {
        "op": "lifecycle_records",
        "tenant_id": tenant_id,
        "query": {"record_kind": record_kind, "phase": phase, "branch": branch},
    }


def list_op(
    tenant_id: str,
    *,
    question_kind: str | None = None,
    resolution_kind: str | None = None,
    from_ms: int | None = None,
    to_ms: int | None = None,
    limit: int = 50,
    cursor: str | None = None,
) -> dict[str, Any]:
    """List the tenant's committed decisions, newest first, one page at a time."""
    op: dict[str, Any] = {"op": "list", "tenant_id": tenant_id, "limit": limit}
    for name, value in (
        ("question_kind", question_kind),
        ("resolution_kind", resolution_kind),
        ("from_ms", from_ms),
        ("to_ms", to_ms),
        ("cursor", cursor),
    ):
        if value is not None:
            op[name] = value
    return op


def provenance_op(tenant_id: str, record_id: str) -> dict[str, Any]:
    """Read the premises, facts, rules and approver one committed decision rested on."""
    return {"op": "provenance", "tenant_id": tenant_id, "record_id": record_id}


def why_not_op(
    tenant_id: str,
    record_id: str,
    option_id: str,
    *,
    budget_ms: int | None = None,
) -> dict[str, Any]:
    """Re-solve why one option was not chosen by a committed assembly record."""
    op: dict[str, Any] = {
        "op": "why_not",
        "tenant_id": tenant_id,
        "record_id": record_id,
        "option_id": option_id,
    }
    if budget_ms is not None:
        op["budget_ms"] = budget_ms
    return op


def learn_op(tenant_id: str, write: str, **fields: Any) -> dict[str, Any]:
    """The one retrieval-learning write (EH-394..EH-397): ``record_outcome``,
    ``fit_adapter``, ``evaluate_generation`` or ``move_pointer``. What was
    learned is read through the decision views' SQL relations, not an op."""
    return {
        "op": "learn",
        "tenant_id": tenant_id,
        "write": {"write": write, **fields},
    }


def capability_outcome_op(
    tenant_id: str,
    entity_id: str,
    decision_id: str,
    *,
    success: bool | None = None,
    reward_q32: int | None = None,
    source_ids: Iterable[str] = (),
    policy: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Record what one run observed of a designated entity (``capability_outcome``):
    exactly one of ``success`` and ``reward_q32`` (a Q32 reward in ``[0, 2**32]``)
    updates the entity's learned reward. A re-send of the same ``decision_id`` and
    outcome applies nothing twice; ``policy`` defaults to the engine default."""
    return {
        "op": "capability_outcome",
        "tenant_id": tenant_id,
        "entity_id": entity_id,
        "success": success,
        "reward_q32": reward_q32,
        "decision_id": decision_id,
        "source_ids": list(source_ids),
        "policy": dict(policy) if policy is not None else {"policy": "default"},
    }


def writeback_op(tenant_id: str, computation: Mapping[str, Any]) -> dict[str, Any]:
    """One ``Writeback`` operation (MEDULLA-DECISION-ENGINE-R615.3): ``computation``
    is a ``kind``-tagged body built by :func:`writeback_queue`,
    :func:`writeback_approve` or :func:`writeback_release`."""
    return {"op": "writeback", "tenant_id": tenant_id, "computation": dict(computation)}


def writeback_proposal(
    proposal_id: str,
    external_id: str,
    system_of_record: str,
    risk_tier: str,
    as_of_ms: int,
    source_values: Mapping[str, str] | None = None,
) -> dict[str, Any]:
    """One proposal to queue with the per-source value digests its preflight reads."""
    return {
        "proposal": {
            "id": proposal_id,
            "external_id": external_id,
            "system_of_record": system_of_record,
            "risk_tier": risk_tier,
            "as_of_ms": as_of_ms,
        },
        "source_values": dict(source_values or {}),
    }


def writeback_queue(
    queue_id: str, proposals: Iterable[Mapping[str, Any]]
) -> dict[str, Any]:
    """Queue preflighted high-tier proposals under ``queue_id``."""
    return {
        "kind": "queue",
        "queue_id": queue_id,
        "proposals": [dict(p) for p in proposals],
    }


def writeback_approve(queue_id: str, proposal_id: str) -> dict[str, Any]:
    """Approve one queued proposal; the approver is the request's verified principal."""
    return {"kind": "approve", "queue_id": queue_id, "proposal_id": proposal_id}


def writeback_release(queue_id: str) -> dict[str, Any]:
    """Release the queue as one governed change set once every proposal is approved."""
    return {"kind": "release", "queue_id": queue_id}


def record_outcome_op(
    tenant_id: str,
    record_id: str,
    returned: Iterable[tuple[str, str | None]],
    cited: Iterable[str],
    *,
    query: Mapping[str, Any] | None = None,
    path: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Attest what a committed retrieval-plan run returned (``(id, content
    class)`` in rank order) and which returned units the answer cited.
    ``query`` (``space_digest`` and ``q16``) is the vector the run ranked with
    and ``path`` the typed plan template it executed; adapter fitting reads
    both."""
    outcome: dict[str, Any] = {
        "record_id": record_id,
        "returned": [
            {"evidence_id": evidence_id, "content_class": content_class}
            for evidence_id, content_class in returned
        ],
        "cited": list(cited),
    }
    if query is not None:
        outcome["query"] = dict(query)
    if path is not None:
        outcome["path"] = dict(path)
    return learn_op(tenant_id, "record_outcome", outcome=outcome)


def belief_revise_request(
    graph: str,
    claim_ids: Sequence[str],
    *,
    texts: Mapping[str, str] | None = None,
    premises: Sequence[Mapping[str, Any]] = (),
    advance: Sequence[Mapping[str, str]] = (),
) -> dict[str, Any]:
    """Build a ``BeliefRevise`` request: the claims whose evidence changed.

    ``texts`` maps a claim id to its text (default: the id), ``premises`` cite
    existing claims as evidence, and ``advance`` moves tracked contradictions to
    their next status (``{"contradiction_id": ..., "to": "proposed"}``).
    """
    return {
        "graph": graph,
        "claim_ids": list(claim_ids),
        "texts": dict(texts or {}),
        "premises": [dict(premise) for premise in premises],
        "advance": [dict(step) for step in advance],
    }


def feature_ledger_open_op(
    feature_id: str, match_ids: Sequence[str] = ()
) -> dict[str, Any]:
    """Record a feature gap in the ledger with the matches it came from."""
    return {"op": "open", "feature_id": feature_id, "match_ids": list(match_ids)}


def feature_ledger_plan_op(
    plan_id: str,
    feature_ids: Sequence[str],
    rationale_digest: str,
    expected_revision: int,
) -> dict[str, Any]:
    """Attach a plan to open entries that are all at ``expected_revision``."""
    return {
        "op": "plan",
        "plan": {
            "plan_id": plan_id,
            "feature_ids": list(feature_ids),
            "rationale_digest": rationale_digest,
        },
        "expected_revision": expected_revision,
    }


def feature_ledger_close_op(
    feature_id: str, closed_by: str, expected_revision: int
) -> dict[str, Any]:
    """Close a planned entry by the change that closed the gap."""
    return {
        "op": "close",
        "feature_id": feature_id,
        "closed_by": closed_by,
        "expected_revision": expected_revision,
    }


def feature_ledger_get_op(feature_ids: Sequence[str] = ()) -> dict[str, Any]:
    """Read entries; no ids reads the whole tenant ledger."""
    return {"op": "get", "feature_ids": list(feature_ids)}


def observer_wire(participant_id: str | None = None) -> str | dict[str, str]:
    """An ``ObserverRef``: ``"system"``, or one participant's own view."""
    if participant_id is None:
        return "system"
    return {"participant": participant_id}


def claim_propose_op(
    claim_id: str,
    *,
    subject: str | None = None,
    observer: str | dict[str, str] = "system",
    statement: str | None = None,
) -> dict[str, Any]:
    """Create a claim in ``proposed``; with a ``subject`` it is about that
    participant as seen by ``observer`` (R652.9), and ``statement`` is the
    sentence it states about them (R655.3)."""
    op: dict[str, Any] = {"op": "propose", "claim_id": claim_id}
    if subject is not None:
        perspective: dict[str, Any] = {"subject": subject, "observer": observer}
        if statement is not None:
            perspective["statement"] = statement
        op["perspective"] = perspective
    return op


def claim_advance_op(claim_id: str, to: str) -> dict[str, Any]:
    """Move a claim along one lifecycle edge (``to`` is the target state)."""
    return {"op": "advance", "claim_id": claim_id, "to": to}


def claim_evidence_op(
    evidence_id: str,
    claim_id: str,
    channel: str | None,
    *,
    supports: bool,
    weight_ppm: int,
    source_digest: str,
    stances: Mapping[str, str] | None = None,
    feed: Mapping[str, Any] | None = None,
    proof: Mapping[str, Any] | None = None,
    session_id: str | None = None,
) -> dict[str, Any]:
    """Record one piece of evidence; ``stances`` maps perspective to stance.

    ``proof`` is a machine-checked proof receipt (R652.10): a receipt must sit on
    the ``process`` channel with ``supports=True`` and moves no log-odds.
    """
    op: dict[str, Any] = {
        "op": "record_evidence",
        "evidence_id": evidence_id,
        "claim_id": claim_id,
        "channel": channel,
        "supports": supports,
        "weight_ppm": weight_ppm,
        "source_digest": source_digest,
        "stances": dict(stances or {}),
        "feed": dict(feed) if feed is not None else None,
    }
    if proof is not None:
        op["proof"] = dict(proof)
    if session_id is not None:
        op["session_id"] = session_id
    return op


def claim_compile_op(
    artifact_id: str,
    cohort_id: str,
    claim_ids: Sequence[str],
    *,
    code_specs: Sequence[str] = (),
    exploration_edges: Sequence[Sequence[str]] = (),
) -> dict[str, Any]:
    """Seal and store a research artifact over a cohort of claims."""
    return {
        "op": "compile_artifact",
        "artifact_id": artifact_id,
        "cohort_id": cohort_id,
        "claim_ids": list(claim_ids),
        "code_specs": list(code_specs),
        "exploration_edges": [list(edge) for edge in exploration_edges],
    }


def claim_get_op(
    *,
    claim_id: str | None = None,
    artifact_id: str | None = None,
    as_observer: str | dict[str, str] | None = None,
) -> dict[str, Any]:
    """Read one claim or one sealed artifact (exactly one id). A claim is read in
    the view the verified principal holds; ``as_observer`` only chooses among the
    participants it acts as."""
    op: dict[str, Any] = {"op": "get", "claim_id": claim_id, "artifact_id": artifact_id}
    if as_observer is not None:
        op["as_observer"] = as_observer
    return op


def claim_proven_op(
    *, axiom: str | None = None, limit: int | None = None
) -> dict[str, Any]:
    """List the claims that hold a machine-checked receipt, one row per claim and
    receipt (R652.13). ``axiom`` keeps the rows whose receipt names it; ``limit``
    defaults to 100 and is at most 1000."""
    op: dict[str, Any] = {"op": "proven_claims"}
    if axiom is not None:
        op["axiom"] = axiom
    if limit is not None:
        op["limit"] = limit
    return op


def profile_share(role: str, action: str, effect: str = "Allow") -> dict[str, Any]:
    """One ``ProfileShare``: ``role`` may (or may not) ``action`` the object."""
    return {"role": role, "action": action, "effect": effect}


def assistant_profile_wire(
    *,
    profile_id: str,
    name: str,
    owner_principal: str,
    instructions: str = "",
    document_set_ids: Sequence[str] = (),
    tool_allowlist: Sequence[str] = (),
    preferred_model_profile_refs: Sequence[str] = (),
    max_context_tokens: int | None = None,
    shares: Sequence[Mapping[str, Any]] = (),
    revision: int = 0,
    tenant: str = "tenant",
) -> dict[str, Any]:
    """An ``AssistantProfile`` in Rust field order. ``revision`` is the stored
    revision the save is derived from (0 for a new profile); the engine writes
    ``revision + 1`` and stamps the caller's tenant."""
    return {
        "profile_id": profile_id,
        "tenant": tenant,
        "name": name,
        "instructions": instructions,
        "document_set_ids": list(document_set_ids),
        "tool_allowlist": list(tool_allowlist),
        "model_hints": {
            "preferred_model_profile_refs": list(preferred_model_profile_refs),
            "max_context_tokens": max_context_tokens,
        },
        "owner_principal": owner_principal,
        "shares": [dict(share) for share in shares],
        "revision": revision,
        "schema_version": 1,
    }


def document_set_wire(
    *,
    set_id: str,
    name: str,
    owner_principal: str,
    members: Sequence[str] | None = None,
    saved_query: str | None = None,
    shares: Sequence[Mapping[str, Any]] = (),
    tenant: str = "tenant",
) -> dict[str, Any]:
    """A ``DocumentSet``: exactly one of ``members`` or ``saved_query``."""
    if (members is None) == (saved_query is None):
        raise ValueError("a document set holds either members or a saved query")
    kind: dict[str, Any] = (
        {"members": list(members or [])}
        if saved_query is None
        else {"saved_query": saved_query}
    )
    return {
        "set_id": set_id,
        "tenant": tenant,
        "name": name,
        "kind": kind,
        "owner_principal": owner_principal,
        "shares": [dict(share) for share in shares],
        "schema_version": 1,
    }


def participant_wire(
    *,
    participant_id: str,
    workspace: str,
    kind: str,
    display_name: str,
    represents: str | None = None,
    acts_as: Sequence[str] = (),
    tenant: str = "tenant",
) -> dict[str, Any]:
    """A ``Participant`` in Rust field order. ``kind`` is ``person``, ``agent``
    or ``object``; a person or agent carries ``represents``. The engine stamps
    the caller's tenant."""
    return {
        "participant_id": participant_id,
        "tenant": tenant,
        "workspace": workspace,
        "kind": kind,
        "display_name": display_name,
        "represents": represents,
        "acts_as": list(acts_as),
        "schema_version": 1,
    }


def put_participant_op(participant: Mapping[str, Any]) -> dict[str, Any]:
    return {"op": "put", "participant": dict(participant)}


def add_member_op(session_id: str, participant_id: str) -> dict[str, Any]:
    return {
        "op": "add_member",
        "session_id": session_id,
        "participant_id": participant_id,
    }


def put_scope_op(
    *,
    scope_id: str,
    name: str,
    owner_principal: str,
    session_ids: Sequence[str] = (),
    shared_with: Sequence[str] = (),
    tenant: str = "tenant",
) -> dict[str, Any]:
    return {
        "op": "put_scope",
        "scope": {
            "scope_id": scope_id,
            "tenant": tenant,
            "name": name,
            "owner_principal": owner_principal,
            "session_ids": sorted(set(session_ids)),
            "shared_with": sorted(set(shared_with)),
            "schema_version": 1,
        },
    }


def set_observation_op(
    session_id: str,
    participant_id: str,
    *,
    observe_self: bool = True,
    observe_others: bool = False,
) -> dict[str, Any]:
    return {
        "op": "set_observation",
        "config": {
            "session_id": session_id,
            "participant_id": participant_id,
            "observe_self": observe_self,
            "observe_others": observe_others,
        },
    }


def complete_derivation_op(payload_ref: str, claims_written: int) -> dict[str, Any]:
    """Finish one derive task: advance the key's checkpoint (R651.12)."""
    return {
        "op": "complete_derivation",
        "payload_ref": payload_ref,
        "claims_written": claims_written,
    }


def participant_get_op(participant_id: str) -> dict[str, Any]:
    """Read one participant the caller acts as (R655.4)."""
    return {"op": "get", "participant_id": participant_id}


def participant_list_op() -> dict[str, Any]:
    """List the participants the caller acts as (R655.4)."""
    return {"op": "list"}


def ask_op(
    subject: str,
    question: str,
    *,
    observer: str | dict[str, str] = "system",
    memory_scope: str | None = None,
) -> dict[str, Any]:
    """Ask about ``subject`` as ``observer`` sees them (R655.2)."""
    body: dict[str, Any] = {
        "subject": subject,
        "observer": observer,
        "question": question,
    }
    if memory_scope is not None:
        body["memory_scope"] = {"scope_id": memory_scope}
    return {"op": "ask", "question": body}


def card_op(
    subject: str, *, observer: str | dict[str, str] = "system"
) -> dict[str, Any]:
    """Read the stored card of ``subject`` as ``observer`` sees them (R655.3)."""
    return {"op": "card", "subject": subject, "observer": observer}


def status_op(memory_scope: str | None = None) -> dict[str, Any]:
    """Read the derivation backlog the caller may see (R655.4)."""
    op: dict[str, Any] = {"op": "status"}
    if memory_scope is not None:
        op["memory_scope"] = {"scope_id": memory_scope}
    return op


def derive_input_op(payload_ref: str) -> dict[str, Any]:
    """Read the turns and perspective of one claimed derive task (R651.12)."""
    return {"op": "derive_input", "payload_ref": payload_ref}


def conclusion_claim_id(claim_prefix: str, conclusion: str) -> str:
    """The claim id a derive worker proposes a conclusion under (R651.12).

    ``claim_prefix`` is the ``claim_prefix`` of the task's ``derive_input``; the
    rest is the SHA-256 of the conclusion's whitespace-normalised, case-folded
    text, so a conclusion that repeats an existing one names the same claim.
    """
    normalised = " ".join(conclusion.split()).lower()
    return claim_prefix + hashlib.sha256(normalised.encode("utf-8")).hexdigest()


def put_profile_op(profile: Mapping[str, Any]) -> dict[str, Any]:
    return {"op": "put_profile", "profile": dict(profile)}


def put_document_set_op(document_set: Mapping[str, Any]) -> dict[str, Any]:
    return {"op": "put_document_set", "set": dict(document_set)}


def get_profile_op(profile_id: str) -> dict[str, Any]:
    return {"op": "get_profile", "profile_id": profile_id}


def get_document_set_op(set_id: str) -> dict[str, Any]:
    return {"op": "get_document_set", "set_id": set_id}


def list_profiles_op() -> dict[str, Any]:
    return {"op": "list_profiles"}


def list_document_sets_op() -> dict[str, Any]:
    return {"op": "list_document_sets"}


def usage_record(
    tenant: str,
    child_id: str,
    operation: str,
    run_id: str,
    recorded_at_ms: int,
    *,
    errored: bool = False,
    model_key: str = "m1",
    principal: str = "",
    tokens_in: int = 100,
    tokens_out: int = 20,
    tokens_cached: int = 0,
    latency_ms: int = 50,
    content: str | None = None,
) -> dict[str, Any]:
    """Build one ``UsageRecord`` for :func:`usage_append_op`.

    Only a SHA-256 digest of ``content`` (default: the run id) travels; the
    engine stamps the verified principal over ``principal``.
    """
    import hashlib

    digest = hashlib.sha256((content or run_id).encode("utf-8")).hexdigest()
    return {
        "schema_version": 1,
        "recorded_at_ms": recorded_at_ms,
        "child_id": child_id,
        "operation": operation,
        "tenant": tenant,
        "principal": principal,
        "run_id": run_id,
        "model_key": model_key,
        "tokens_in": tokens_in,
        "tokens_out": tokens_out,
        "tokens_cached": tokens_cached,
        "latency_ms": latency_ms,
        "outcome": "error" if errored else "ok",
        "content_digest": digest,
    }


def correction_record_op(
    correction_id: str,
    target: str,
    before: str,
    after: str,
    at_ms: int,
    *,
    source: str | None = None,
) -> dict[str, Any]:
    """A ``CorrectionRecord`` record of one correction of ``target``, the node id of the
    action record being corrected (the verified role ``corrector`` is required). The
    engine reads the producer and the task class from that record and takes the
    corrector from the verified principal; the caller names neither. ``source`` is
    ``human`` (the default), ``review_comment`` (``before`` and ``after`` are
    ``sha256:`` digests, never the comment text) or ``review_rejection`` (R702.7)."""
    correction: dict[str, Any] = {
        "correction_id": correction_id,
        "target": target,
        "before": before,
        "after": after,
        "at_ms": at_ms,
    }
    if source is not None:
        correction["source"] = source
    return {"op": "record", "correction": correction}


def correction_nearest_op(target: str) -> dict[str, Any]:
    """A ``CorrectionRecord`` read of the nearest prior corrections to ``target``."""
    return {"op": "nearest", "target": target}


def correction_get_op(receipt_id: str) -> dict[str, Any]:
    """A ``CorrectionRecord`` read of one correction by ``receipt_id``."""
    return {"op": "get", "receipt_id": receipt_id}


def correction_outcomes_op(rule_id: str | None = None) -> dict[str, Any]:
    """A ``CorrectionRecord`` read of the independent outcomes per rule candidate,
    optionally narrowed to one ``rule_id`` (R702.8)."""
    op: dict[str, Any] = {"op": "outcomes"}
    if rule_id is not None:
        op["rule_id"] = rule_id
    return op


def failure_ingest_op(graph: str, since_ms: int | None = None) -> dict[str, Any]:
    """A ``FailureIngest`` ingest of the failed runs in ``graph`` (optionally since
    ``since_ms``)."""
    op: dict[str, Any] = {"op": "ingest", "graph": graph}
    if since_ms is not None:
        op["since_ms"] = since_ms
    return op


def knowledge_flag_raise_op(
    graph: str,
    knowledge_kind: str,
    flag_kind: str,
    target_ref: str,
    what_observed: str,
    run_id: str,
    at_ms: int,
    suggested_correction: str | None = None,
    trace_node_id: str | None = None,
) -> dict[str, Any]:
    """A ``KnowledgeFlag`` raise (scope ``knowledge:flag``).

    ``knowledge_kind`` is ``skill``, ``domain`` or ``data_access``; ``flag_kind`` is
    ``missing`` or ``wrong``; ``target_ref`` is ``skill:<name>@<revision>``,
    ``term:<iri>``, ``source:<source_id>`` or ``topic:<slug>``.
    """
    op: dict[str, Any] = {
        "op": "raise",
        "graph": graph,
        "knowledge_kind": knowledge_kind,
        "flag_kind": flag_kind,
        "target_ref": target_ref,
        "what_observed": what_observed,
        "run_id": run_id,
        "at_ms": at_ms,
    }
    if suggested_correction is not None:
        op["suggested_correction"] = suggested_correction
    if trace_node_id is not None:
        op["trace_node_id"] = trace_node_id
    return op


def knowledge_flag_resolve_op(
    graph: str,
    flag_id: str,
    resolution: str,
    correction_id: str | None = None,
    fixed_by: str | None = None,
) -> dict[str, Any]:
    """A ``KnowledgeFlag`` resolve (scope ``knowledge:curate``): ``resolution`` is
    ``adopt`` or ``dismiss``."""
    op: dict[str, Any] = {
        "op": "resolve",
        "graph": graph,
        "flag_id": flag_id,
        "resolution": resolution,
    }
    if correction_id is not None:
        op["correction_id"] = correction_id
    if fixed_by is not None:
        op["fixed_by"] = fixed_by
    return op


def knowledge_flag_get_op(graph: str, flag_id: str) -> dict[str, Any]:
    """A ``KnowledgeFlag`` read of one flag with the nodes it links to."""
    return {"op": "get", "graph": graph, "flag_id": flag_id}


def knowledge_flag_list_op(
    graph: str, state: str | None = None, limit: int | None = None
) -> dict[str, Any]:
    """A ``KnowledgeFlag`` list, newest first, optionally of one ``state``."""
    op: dict[str, Any] = {"op": "list", "graph": graph}
    if state is not None:
        op["state"] = state
    if limit is not None:
        op["limit"] = limit
    return op


def knowledge_curation_inputs_op(
    graph: str,
    loop_id: str,
    epoch: int,
    force: bool = False,
    cursor: str | None = None,
    limit: int | None = None,
) -> dict[str, Any]:
    """A ``KnowledgeCuration`` read of the curation inputs (scope ``knowledge:curate``,
    live lease of ``loop_id`` at ``epoch``)."""
    op: dict[str, Any] = {
        "op": "inputs",
        "graph": graph,
        "loop_id": loop_id,
        "epoch": epoch,
        "force": force,
    }
    if cursor is not None:
        op["cursor"] = cursor
    if limit is not None:
        op["limit"] = limit
    return op


def knowledge_curation_propose_op(
    graph: str,
    loop_id: str,
    epoch: int,
    proposal: Mapping[str, Any],
    body: str,
) -> dict[str, Any]:
    """A ``KnowledgeCuration`` proposal: the edit, the flags it answers and its body."""
    return {
        "op": "propose",
        "graph": graph,
        "loop_id": loop_id,
        "epoch": epoch,
        "proposal": dict(proposal),
        "body": body,
    }


def knowledge_curation_status_op(graph: str, proposal_id: str) -> dict[str, Any]:
    """A ``KnowledgeCuration`` status read; it closes an adopted proposal's flags."""
    return {"op": "status", "graph": graph, "proposal_id": proposal_id}


def knowledge_curation_metrics_op(
    graph: str, from_ms: int, to_ms: int
) -> dict[str, Any]:
    """A ``KnowledgeCuration`` read of the closed-loop measures of a window."""
    return {"op": "metrics", "graph": graph, "from_ms": from_ms, "to_ms": to_ms}


def friction_report_op(graph: str, report: Mapping[str, Any]) -> dict[str, Any]:
    """A ``FrictionReport`` report of one skill's friction (scope ``knowledge:flag``);
    the engine overwrites ``tenant_id`` and ``reported_by`` with the verified caller."""
    return {"op": "report", "graph": graph, "report": dict(report)}


def friction_transition_op(
    graph: str,
    group_ref: str,
    to: str,
    at_ms: int,
    evidence_ref: str | None = None,
) -> dict[str, Any]:
    """A ``FrictionReport`` move of a group (scope ``knowledge:curate``, or
    ``knowledge:approve`` into ``approved``)."""
    transition: dict[str, Any] = {"group_ref": group_ref, "to": to}
    if evidence_ref is not None:
        transition["evidence_ref"] = evidence_ref
    return {
        "op": "transition",
        "graph": graph,
        "transition": transition,
        "at_ms": at_ms,
    }


def friction_group_op(
    graph: str, skill_ref: str | None = None, limit: int | None = None
) -> dict[str, Any]:
    """A ``FrictionReport`` list of groups by report count, most first."""
    op: dict[str, Any] = {"op": "group", "graph": graph}
    if skill_ref is not None:
        op["skill_ref"] = skill_ref
    if limit is not None:
        op["limit"] = limit
    return op


def skill_pseudonym(tenant_id: str, principal: str, salt: int = 1) -> str:
    """The pseudonym of one principal in skill telemetry: the hex SHA-256 the engine
    frames over the tenant, the principal and knob ``skill.pseudonym_salt``."""
    import hashlib

    domain = b"medulla.skill_telemetry.pseudonym.v1"
    fields = [tenant_id.encode(), principal.encode(), str(salt).encode()]
    body = b"eg/framed-sha256/v1\0" + len(domain).to_bytes(4, "big") + domain
    body += len(fields).to_bytes(4, "big")
    for item in fields:
        body += len(item).to_bytes(8, "big") + item
    return hashlib.sha256(body).hexdigest()


def skill_telemetry_ingest_op(
    graph: str, events: Sequence[Mapping[str, Any]]
) -> dict[str, Any]:
    """A ``SkillTelemetry`` ingest of at most 500 invocations (``telemetry:write``);
    one bad event refuses the whole batch."""
    return {"op": "ingest", "graph": graph, "events": [dict(event) for event in events]}


def skill_telemetry_measures_op(
    graph: str, skill_ref: str, start_ms: int, end_ms: int
) -> dict[str, Any]:
    """A ``SkillTelemetry`` read of one skill's twelve measures (``telemetry:read``)."""
    return {
        "op": "measures",
        "graph": graph,
        "skill_ref": skill_ref,
        "window": {"start_ms": start_ms, "end_ms": end_ms},
    }


def skill_telemetry_rank_op(
    graph: str,
    start_ms: int,
    end_ms: int,
    skill_refs: list[str] | None = None,
    limit: int | None = None,
) -> dict[str, Any]:
    """A ``SkillTelemetry`` quality rank of skills."""
    op: dict[str, Any] = {
        "op": "rank",
        "graph": graph,
        "skill_refs": list(skill_refs or []),
        "window": {"start_ms": start_ms, "end_ms": end_ms},
    }
    if limit is not None:
        op["limit"] = limit
    return op


def skill_telemetry_peers_op(
    graph: str, cohort: list[str], start_ms: int, end_ms: int
) -> dict[str, Any]:
    """A ``SkillTelemetry`` read of the skills a cohort used and the caller did not;
    a cohort under the floor of ``skill.peers.min_peers`` is refused."""
    return {
        "op": "peers",
        "graph": graph,
        "cohort": list(cohort),
        "window": {"start_ms": start_ms, "end_ms": end_ms},
    }


def skill_telemetry_traces_op(
    graph: str,
    skill_ref: str,
    outcome: str,
    limit: int | None = None,
    cursor: str | None = None,
) -> dict[str, Any]:
    """A ``SkillTelemetry`` read of the run traces of a skill by eval outcome
    (``pass``, ``fail`` or ``unscored``)."""
    op: dict[str, Any] = {
        "op": "traces_by_eval",
        "graph": graph,
        "skill_ref": skill_ref,
        "outcome": outcome,
    }
    if limit is not None:
        op["limit"] = limit
    if cursor is not None:
        op["cursor"] = cursor
    return op


def friction_get_op(graph: str, group_ref: str) -> dict[str, Any]:
    """A ``FrictionReport`` read of one group with its reports and steps."""
    return {"op": "get", "graph": graph, "group_ref": group_ref}


def usage_append_op(records: Iterable[Mapping[str, Any]]) -> dict[str, Any]:
    """A ``Usage`` append of ``records`` (scope ``usage:write``)."""
    return {"op": "append", "records": [dict(record) for record in records]}


def usage_read_op(
    from_ms: int,
    to_ms: int,
    view: str,
    model_key: str | None = None,
    cursor: str | None = None,
) -> dict[str, Any]:
    """A ``Usage`` read of the caller's tenant (scope ``usage:read``).

    ``view`` is ``records`` (paged by ``cursor``) or ``rollup`` (day buckets).
    """
    op: dict[str, Any] = {
        "op": "read",
        "from_ms": from_ms,
        "to_ms": to_ms,
        "view": view,
    }
    if model_key is not None:
        op["model_key"] = model_key
    if cursor is not None:
        op["cursor"] = cursor
    return op


def usage_put_price_op(catalog: Mapping[str, Any]) -> dict[str, Any]:
    """A ``Usage`` price put of ``catalog`` (scope ``usage:write``)."""
    return {
        "op": "put_price",
        "catalog": {
            "aliases": dict(catalog.get("aliases", {})),
            "entries": [dict(entry) for entry in catalog.get("entries", ())],
        },
    }


def _payload(result: Any) -> Any:
    return getattr(result, "payload", result)


@dataclass(frozen=True, slots=True)
class DecisionClient:
    """``Decide`` and ``DecisionLog`` over the generated senders."""

    client: Any
    graph: str | None = None

    async def decide(
        self, request: Mapping[str, Any], *, as_model: bool = False
    ) -> Any:
        """Send ``Decide``.

        ``as_model=True`` opts into the generated, nested Pydantic
        ``DecisionBatch`` model (its records, outcomes and resolution/evidence
        markers all typed) instead of the raw payload. Default is ``False``:
        this is a decision-type-scoped pilot (MEDULLA-DECISION-ENGINE-R076), not a
        change to this client's general opaque-result behavior.
        """
        from medulla.generated.query import decode_decide, send_decide

        result = await send_decide(self.client, {"request": dict(request)}, self.graph)
        if as_model:
            return decode_decide(result)
        return _payload(result)

    async def designate(self, request: Mapping[str, Any]) -> Any:
        """Send a ``capability_designation`` ``Decide`` built by
        :func:`capability_designation_request`."""
        return await self.decide(request)

    async def log(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        from medulla.generated.coordination import send_decision_log

        result = await send_decision_log(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def fit(
        self, request: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``DecisionFit`` ``submit`` for one fit request.

        The result is the job row the engine answers with, unchanged. The
        caller needs the ``admin:decision-fit`` scope.
        """
        from medulla.generated.coordination import send_decision_fit

        result = await send_decision_fit(
            self.client,
            {"op": {"op": "submit", "request": dict(request)}},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def fit_status(self, request: Mapping[str, Any]) -> Any:
        """Send ``DecisionFit`` ``status`` for one submitted job.

        ``request`` is ``{"tenant_id": ..., "job_id": ...}``; the result is
        the job row, unchanged.
        """
        from medulla.generated.coordination import send_decision_fit

        result = await send_decision_fit(
            self.client,
            {"op": {"op": "status", "request": dict(request)}},
            self.graph,
        )
        return _payload(result)

    async def revision_proposal_list(
        self, *, cursor: str | None = None, limit: int | None = None
    ) -> Any:
        """Send ``RevisionProposalList``: the verified tenant's revision rows.

        The request carries no tenant; the engine pages the caller's own rows in
        creation order. Pass the previous page's ``next_cursor`` to continue.
        """
        from medulla.generated.storage import send_revision_proposal_list

        request: dict[str, Any] = {}
        if cursor is not None:
            request["cursor"] = cursor
        if limit is not None:
            request["limit"] = limit
        result = await send_revision_proposal_list(
            self.client, {"request": request}, self.graph
        )
        return _payload(result)

    async def claims(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``Claims`` (research claims, evidence and sealed artifacts)."""
        from medulla.generated.coordination import send_claims

        result = await send_claims(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def loops(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``Loops`` (fenced loop leases, gaps, offers, ledger and ratchet)."""
        from medulla.generated.coordination import send_loops

        result = await send_loops(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def usage(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``Usage`` (append usage records, put a price catalog or read the
        ledger)."""
        from medulla.generated.coordination import send_usage

        result = await send_usage(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def correction_record(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``CorrectionRecord`` (record a human correction with its three effects,
        read the nearest corrections, or read one by receipt id)."""
        from medulla.generated.coordination import send_correction_record

        result = await send_correction_record(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def skill_eval(
        self, request: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``DecisionEval`` op ``skill_eval`` (run one skill's suite)."""
        from medulla.generated.coordination import send_decision_eval

        result = await send_decision_eval(
            self.client,
            {"op": skill_eval_op(request)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def failure_ingest(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``FailureIngest`` (cluster failed runs and file one gap per recurring
        pattern)."""
        from medulla.generated.coordination import send_failure_ingest

        result = await send_failure_ingest(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def knowledge_flag(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``KnowledgeFlag`` (raise, resolve, get or list a knowledge flag)."""
        from medulla.generated.coordination import send_knowledge_flag

        result = await send_knowledge_flag(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def knowledge_curation(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``KnowledgeCuration`` (curation inputs, propose, status or metrics)."""
        from medulla.generated.coordination import send_knowledge_curation

        result = await send_knowledge_curation(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def skill_telemetry(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``SkillTelemetry`` (ingest invocations, or read measures, rank, peers
        and traces by eval)."""
        from medulla.generated.coordination import send_skill_telemetry

        result = await send_skill_telemetry(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def friction_report(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``FrictionReport`` (file a report, move, list or read a group)."""
        from medulla.generated.coordination import send_friction_report

        result = await send_friction_report(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def belief_revise(
        self, request: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``BeliefRevise`` (belief revision over claims with new evidence)."""
        from medulla.generated.coordination import send_belief_revise

        result = await send_belief_revise(
            self.client,
            {"request": dict(request)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def feature_ledger(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``FeatureLedger`` (open, plan, close and read feature gaps)."""
        result = await self.client._send(
            "FeatureLedger",
            {"op": dict(op)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def assistant_profile(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``AssistantProfile`` (save, share, read profiles and document sets)."""
        from medulla.generated.coordination import send_assistant_profile

        result = await send_assistant_profile(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def participant(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``Participant``: put a participant, add a session member, put a
        memory scope or set a participant's observation flags."""
        from medulla.generated.coordination import send_participant

        result = await send_participant(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def participant_read(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``ParticipantRead``: get or list participants, ask about one,
        read a card or the derivation status, or read a derive task's input."""
        from medulla.generated.coordination import send_participant_read

        result = await send_participant_read(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)

    async def semantic_enrich(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``SemanticEnrich``: one scheduled card, ranking, classification
        or projection. ``op`` names its ``schedule_record_id``."""
        result = await self.client._send(
            "SemanticEnrich",
            {"op": dict(op)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def harness_edit(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``HarnessEdit``: ``{"op": "create", "edit_id": "...", "dimension_iri":
        "...", "round": 1}`` writes the edit as ``proposed`` with its
        ``targetsDimension`` edge (the only way to create one: a generic write naming
        ``editStatus`` is refused ``EDIT_STATUS_ENGINE_OWNED``), ``{"op": "transition",
        "edit_id": "...", "event": "ship" | "revert"}`` applies one event of the edit
        machine to a stored harness edit, ``{"op": "get", "edit_id": "..."}`` reads its
        recorded status."""
        result = await self.client._send(
            "HarnessEdit",
            {"op": dict(op)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def observation_state(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``ObservationState``: ``{"op": "put", "record": {...}}`` stores one
        run-bound observation, ``{"op": "get", "key": {...}}`` reads it while live."""
        result = await self.client._send(
            "ObservationState",
            {"op": dict(op)},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def memory_remember(
        self,
        record: Mapping[str, Any],
        embedding: Sequence[float] = (),
        *,
        episode: Mapping[str, Any] | None = None,
        idempotency_key: str | None = None,
    ) -> Any:
        """Send ``MemoryRemember``: store one memory record and the embedding it is
        recalled by under the verified tenant. ``episode`` (``episode_id``,
        ``session_id`` and the ``preceding`` record ids) seals the record and the
        preceding ones into one immutable episode."""
        request: dict[str, Any] = {
            "record": dict(record),
            "embedding": [float(v) for v in embedding],
        }
        if episode is not None:
            request["episode"] = dict(episode)
        result = await self.client._send(
            "MemoryRemember",
            {"request": request},
            self.graph,
            idempotency_key=idempotency_key,
        )
        return _payload(result)

    async def memory_recall(self, request: Mapping[str, Any]) -> Any:
        """Send ``MemoryRecall``: rank the tenant's memory inside ``budget`` tokens;
        ``prime_session`` adds session mementos and ``include_plan`` a consolidation
        plan."""
        result = await self.client._send(
            "MemoryRecall", {"request": dict(request)}, self.graph
        )
        return _payload(result)

    async def memory_export(self, limit: int | None = None) -> Any:
        """Send ``MemoryExport``: the procedural and semantic tiers with a manifest."""
        result = await self.client._send(
            "MemoryExport", {"request": {"limit": limit}}, self.graph
        )
        return _payload(result)


@dataclass(frozen=True, slots=True)
class MeasuredFactRecorded:
    """What :func:`record_measured_fact` did: the profile component, its revision
    after the call, and whether the fact was already the head's (no write)."""

    component_id: str
    entry_revision: int
    replayed: bool


_MEASURED_FACT_FIELDS = (
    "shell_proficiency",
    "stopping_behaviour",
    "context_window_tokens",
)
_PLACEHOLDER = "placeholder"


def _measured_fact_context(client: Any, expected_revision: int) -> dict[str, Any]:
    """The mutation context of a library write. Every field but the tenant and
    the expected revision is a placeholder the server replaces from the verified
    request (``bind_agent_library_context``), never from the body."""
    principal = "principal:sha256:" + "a" * 64
    tenant = getattr(client, "tenant", None) or client._request_tenant()
    return {
        "request_id": 0,
        "principal": principal,
        "caller_principal": principal,
        "attempt_nonce": uuid.uuid4().hex * 2,
        "tenant_id": tenant,
        "actor_scope": _PLACEHOLDER,
        "purpose_id": _PLACEHOLDER,
        "policy_revision": _PLACEHOLDER,
        "policy_digest": "sha256:" + "0" * 64,
        "policy_decision_id": _PLACEHOLDER,
        "idempotency_key": uuid.uuid4().hex,
        "expected_revision": expected_revision,
        "trace_id": None,
        "created_at_ms": 0,
    }


async def record_measured_fact(
    client: Any,
    model_profile_ref: str,
    field: Literal["shell_proficiency", "stopping_behaviour", "context_window_tokens"],
    value: str | int,
    evaluation_id: str,
    digest: str,
) -> MeasuredFactRecorded:
    """Apply one evaluation result once through the ``ModelProfile`` publish
    (MEDULLA-ORCH-VOCAB-R011.10).

    Reads the head, copies it into the next draft with the one fact changed and
    publishes it. A head that already holds the fact returns ``replayed=True``
    without sending anything, so a retried result adds no revision. A concurrent
    head move or an invalid run reference is the engine's refusal, unchanged.
    """
    from medulla.generated.agent_component import AgentComponentDraft
    from medulla.generated.storage import send_agent_component

    if field not in _MEASURED_FACT_FIELDS:
        raise ValueError(f"UNKNOWN_MEASURED_FACT: {field}")
    tenant = getattr(client, "tenant", None) or client._request_tenant()
    head = (
        await send_agent_component(
            client,
            {
                "op": {
                    "op": "current",
                    "tenant_id": tenant,
                    "component_id": model_profile_ref,
                }
            },
        )
    ).payload
    if not isinstance(head, dict) or head.get("kind") != "model_profile":
        raise ValueError(f"MODEL_PROFILE_UNKNOWN: {model_profile_ref}")

    if field == "context_window_tokens":
        fact: Any = value
    else:
        fact = {
            "value": value,
            "evaluation_run_ref": {"evaluation_id": evaluation_id, "digest": digest},
        }
    facts = dict(head["facts"])
    if facts.get(field) == fact:
        return MeasuredFactRecorded(
            component_id=head["component_id"],
            entry_revision=head["entry_revision"],
            replayed=True,
        )
    facts[field] = fact
    draft = {
        name: head[name] for name in AgentComponentDraft.model_fields if name in head
    }
    draft["facts"] = facts
    result = await send_agent_component(
        client,
        {
            "op": {
                "op": "publish",
                "request": {
                    "context": _measured_fact_context(client, head["entry_revision"]),
                    "component": draft,
                    "evaluation_receipt_digest": None,
                },
            }
        },
        idempotency_key=f"measured-fact:{model_profile_ref}:{field}:{evaluation_id}",
    )
    payload = result.payload
    return MeasuredFactRecorded(
        component_id=model_profile_ref,
        entry_revision=payload["entry"]["entry_revision"],
        replayed=False,
    )


__all__ = [
    "MeasuredFactRecorded",
    "Q32_ONE",
    "TASK_PLAN_FEATURE_SCHEMA",
    "DecisionClient",
    "DeclaredOption",
    "add_member_op",
    "observer_wire",
    "aggregate_op",
    "assistant_profile_wire",
    "belief_revise_request",
    "claim_advance_op",
    "claim_compile_op",
    "claim_evidence_op",
    "capability_outcome_op",
    "claim_get_op",
    "claim_proven_op",
    "claim_propose_op",
    "commit_op",
    "correction_get_op",
    "correction_outcomes_op",
    "correction_nearest_op",
    "correction_record_op",
    "decide_request",
    "document_set_wire",
    "declared",
    "evaluate_op",
    "failure_ingest_op",
    "feature_ledger_close_op",
    "feature_ledger_get_op",
    "feature_ledger_open_op",
    "feature_ledger_plan_op",
    "get_document_set_op",
    "get_op",
    "get_profile_op",
    "friction_get_op",
    "skill_pseudonym",
    "skill_telemetry_ingest_op",
    "skill_telemetry_measures_op",
    "skill_telemetry_peers_op",
    "skill_telemetry_rank_op",
    "skill_telemetry_traces_op",
    "friction_group_op",
    "friction_report_op",
    "friction_transition_op",
    "knowledge_curation_inputs_op",
    "knowledge_curation_metrics_op",
    "knowledge_curation_propose_op",
    "knowledge_curation_status_op",
    "knowledge_flag_get_op",
    "knowledge_flag_list_op",
    "knowledge_flag_raise_op",
    "knowledge_flag_resolve_op",
    "learn_op",
    "list_document_sets_op",
    "list_profiles_op",
    "library",
    "named_evaluator",
    "param",
    "participant_wire",
    "ask_op",
    "card_op",
    "complete_derivation_op",
    "derive_input_op",
    "participant_get_op",
    "participant_list_op",
    "status_op",
    "profile_share",
    "put_document_set_op",
    "put_participant_op",
    "put_profile_op",
    "put_scope_op",
    "q32_of",
    "record_measured_fact",
    "record_outcome_op",
    "resolve_op",
    "certify_local_skill_op",
    "promote_rule_op",
    "propose_rule_op",
    "approve_rule_op",
    "revoke_rule_op",
    "policy_activation_op",
    "set_observation_op",
    "skill_eval_op",
    "task_plan_request",
    "verify_task_plan_receipt",
    "usage_append_op",
    "usage_put_price_op",
    "usage_read_op",
    "usage_record",
]
