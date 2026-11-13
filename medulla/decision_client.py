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

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

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
    from medulla.generated.models import TaskPlanProposal

    typed = TaskPlanProposal.model_validate(dict(proposal), strict=True)
    # ``TaskPlanTask::call`` carries ``skip_serializing_if = "Option::is_none"``
    # on the engine side, so an absent call is an absent key -- never ``null``.
    # The generated senders dump the same way (``exclude_none=True``).
    body = typed.model_dump(mode="json", exclude_none=True)
    return decide_request(
        tenant_id,
        (question_id, "task_plan", "ordinary"),
        declared(DeclaredOption(task["task_id"]) for task in body["tasks"]),
        feature_schema,
        policy=policy,
        params=[param("task_plan", "task_plan", body)],
        max_records=1,
    )


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
            "arguments": call["arguments"],
        }
    aggregation = task.get("aggregation")
    if aggregation is not None:
        body["aggregation"] = {"op": aggregation["op"]}
        if aggregation.get("reducer_id") is not None:
            body["aggregation"]["reducer_id"] = aggregation["reducer_id"]
    if task.get("assignee") is not None:
        body["assignee"] = task["assignee"]
    _add_control_steps(body, task)
    return body


def _condition_body(condition: Mapping[str, Any]) -> dict[str, Any]:
    if condition.get("decision_iri") is not None:
        return {"decision_iri": condition["decision_iri"]}
    return {
        "ref": condition["ref"],
        "op": condition["op"],
        "value": condition["value"],
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


def _task_plan_body(value: Mapping[str, Any]) -> dict[str, Any]:
    """A validated proposal in Rust declaration order, for its digest."""
    return {
        "tasks": [_task_plan_task(task) for task in value["tasks"]],
        "output_task_id": value["output_task_id"],
        "max_concurrency": value["max_concurrency"],
        "max_steps": value["max_steps"],
    }


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
        == {"outcome": "advisory", "scores": [], "calibrated": False},
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
    _require_task_plan(
        plan["plan_digest"]
        == digest_text(
            "eg/task-plan/v1",
            [
                body["tasks"],
                body["output_task_id"],
                body["max_concurrency"],
                body["max_steps"],
                plan["total_tasks"],
                plan["policy_revision"],
                plan["policy_digest"],
            ],
        ),
        "plan digest",
    )
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
    if served_plan is None:
        raise ValueError("task_plan receipt mismatch: missing plan")
    plan = served_plan.model_dump(mode="json", by_alias=True, exclude_none=True)
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


def evaluate_op(tenant_id: str, evaluation: Mapping[str, Any]) -> dict[str, Any]:
    return {"op": "evaluate", "tenant_id": tenant_id, "evaluation": dict(evaluation)}


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


def learn_op(tenant_id: str, write: str, **fields: Any) -> dict[str, Any]:
    """The one retrieval-learning write (EH-394..EH-397): ``record_outcome``,
    ``fit_adapter``, ``evaluate_generation`` or ``move_pointer``. What was
    learned is read through the decision views' SQL relations, not an op."""
    return {
        "op": "learn",
        "tenant_id": tenant_id,
        "write": {"write": write, **fields},
    }


def record_outcome_op(
    tenant_id: str,
    record_id: str,
    returned: Iterable[tuple[str, str | None]],
    cited: Iterable[str],
) -> dict[str, Any]:
    """Attest what a committed retrieval-plan run returned (``(id, content
    class)`` in rank order) and which returned units the answer cited."""
    outcome = {
        "record_id": record_id,
        "returned": [
            {"evidence_id": evidence_id, "content_class": content_class}
            for evidence_id, content_class in returned
        ],
        "cited": list(cited),
    }
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


def claim_propose_op(claim_id: str) -> dict[str, Any]:
    """Create a claim in ``proposed``."""
    return {"op": "propose", "claim_id": claim_id}


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
) -> dict[str, Any]:
    """Record one piece of evidence; ``stances`` maps perspective to stance."""
    return {
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
    *, claim_id: str | None = None, artifact_id: str | None = None
) -> dict[str, Any]:
    """Read one claim or one sealed artifact (exactly one id)."""
    return {"op": "get", "claim_id": claim_id, "artifact_id": artifact_id}


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

    async def log(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        from medulla.generated.coordination import send_decision_log

        result = await send_decision_log(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
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

    async def assistant_profile(
        self, op: Mapping[str, Any], *, idempotency_key: str | None = None
    ) -> Any:
        """Send ``AssistantProfile`` (save, share, read profiles and document sets)."""
        from medulla.generated.coordination import send_assistant_profile

        result = await send_assistant_profile(
            self.client, {"op": dict(op)}, self.graph, idempotency_key=idempotency_key
        )
        return _payload(result)


__all__ = [
    "Q32_ONE",
    "TASK_PLAN_FEATURE_SCHEMA",
    "DecisionClient",
    "DeclaredOption",
    "aggregate_op",
    "assistant_profile_wire",
    "belief_revise_request",
    "claim_advance_op",
    "claim_compile_op",
    "claim_evidence_op",
    "claim_get_op",
    "claim_propose_op",
    "commit_op",
    "decide_request",
    "document_set_wire",
    "declared",
    "evaluate_op",
    "get_document_set_op",
    "get_op",
    "get_profile_op",
    "learn_op",
    "list_document_sets_op",
    "list_profiles_op",
    "library",
    "named_evaluator",
    "param",
    "profile_share",
    "put_document_set_op",
    "put_profile_op",
    "q32_of",
    "record_outcome_op",
    "resolve_op",
    "task_plan_request",
    "verify_task_plan_receipt",
]
