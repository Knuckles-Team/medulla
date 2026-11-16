"""MEDULLA-DECISION-ENGINE-R803.3/.6 served receipts: the contrastive harness
composition decision over the wire.

The entry point is the served ``Decide`` method with the
``harness_contrast`` question kind: the caller declares each candidate
component version as an option (option id = the version's node id, the probe
trace id as a declared text) and names the task, the task class, the baseline
trace, the identical probe conditions and the evidence graph as typed request
params. The engine reads the held versions and the probe runs out of the named
graph and measures the four deltas itself (R803.7): each run's trace committed
through ``DecisionLog``, its independent ``:Score`` rows linked by
``scoresRun`` and its priced cost in the usage ledger. It then adopts the
improving candidate (positive ``eval_score_delta`` no other candidate beats,
cost the tie-break) or abstains with
``no_improvement`` when nothing improves, and seals the answer as one record
whose ``inputs`` carry the decision. R803.3 commits that record through
``DecisionLog`` -- the commit runs verify-replay over the contrast's own
deterministic core -- and reads it back by record id.

The graph evidence (published component versions, probe runs, score rows) is
written through the generated client write
(``ApplyChangeEnvelopes``; ``changes.apply_batch`` compiles to it), the same
entry point as the R801/R802 served tests.
"""

from __future__ import annotations

import asyncio
import os
import time
import uuid
from collections.abc import Sequence
from typing import Any, NamedTuple

import msgpack
import pytest
from _decision_resident_fixture import publish
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient
from medulla.decision import digest_text

Q32 = 1 << 32
#: The two components whose versions the contrast candidates are: one
#: version per component, so each publishes as its component's first
#: version without a supersession of its own.
COMPONENT_A = "comp:contrast-a"
COMPONENT_B = "comp:contrast-b"
CONTENT_DIGEST_DOMAIN = "eg/harness-component-content/v1"
TASK_REF = "task:h2-r803"
TASK_CLASS = "eg:taskclass/contrast"
CONDITIONS_DIGEST = "sha256:conditions"
#: The model key the probe runs' usage is priced under: one micro-unit per
#: million input tokens, so a run's ledger cost is its token count / 10^6.
PROBE_MODEL = "contrast-probe"
#: The four contrast delta features, in the sorted order the decision
#: record's feature matrix names them.
CONTRAST_FEATURES = [
    "cost_delta",
    "eval_score_delta",
    "retry_delta",
    "tool_selection_delta",
]


def content_digest(content_ref: str) -> str:
    """The R801 vocabulary's content digest -- the engine's one digest scheme."""
    return digest_text(CONTENT_DIGEST_DOMAIN, content_ref)


def version_node_id(component: str, content_ref: str) -> str:
    return f"harness-component:{component}:{content_digest(content_ref)}"


def version_props(
    component: str, content_ref: str, valid_from: int, tx_from: int
) -> dict[str, Any]:
    """One published version as the writer composes it: the record's fields
    plus the engine-owned write marker and the client's transaction stamp."""
    return {
        "_harness_component": True,
        "component_id": component,
        "kind": "system_prompt",
        "content_ref": content_ref,
        "content_digest": content_digest(content_ref),
        "valid_from": valid_from,
        "confidence": 0.8,
        "evidence_run_id": f"run:{component}:{content_ref}",
        "applied_to_id": "agent:h2-r803",
        "tx_from": tx_from,
    }


def probe_run_node_id(trace_id: str) -> str:
    return f"harness-probe-run:{trace_id}"


def probe_run_props(trace_id: str, tx_from: int) -> dict[str, Any]:
    return {
        "_harness_probe_run": True,
        "trace_id": trace_id,
        "conditions_digest": CONDITIONS_DIGEST,
        "tx_from": tx_from,
    }


class ProbeRun(NamedTuple):
    """One probe run as the engine measures it: ``retries`` failed tool calls
    each retried once, ``extra_ok`` further clean tool calls, one independent
    score on a 0..1 range and the run's ledger cost in micro-units."""

    name: str
    retries: int
    extra_ok: int
    score: float
    cost: int


def _trace(run_id: str, run: ProbeRun) -> dict[str, Any]:
    steps: list[tuple[str, str]] = []
    for index in range(run.retries):
        steps += [(f"tool:retry-{index}", "error"), (f"tool:retry-{index}", "ok")]
    steps += [(f"tool:clean-{index}", "ok") for index in range(run.extra_ok)]
    return {
        "schema_version": 1,
        "run_id": run_id,
        "task_signature": TASK_REF,
        "model_id": PROBE_MODEL,
        "steps": [
            {
                "index": index,
                "kind": "tool_call",
                "action_ref": action,
                "outcome": outcome,
            }
            for index, (action, outcome) in enumerate(steps)
        ],
        "outcome": "succeeded",
        "started_at_ms": 1,
    }


def _q32_of_micros(micros: int) -> int:
    """The engine's millionths-to-Q32 conversion (truncates toward zero)."""
    magnitude = (abs(micros) << 32) // 1_000_000
    return magnitude if micros >= 0 else -magnitude


def measured_deltas(baseline: ProbeRun, run: ProbeRun) -> dict[str, int]:
    """The four deltas the engine measures for ``run`` against ``baseline``."""
    return {
        "retry_delta": (run.retries - baseline.retries) * Q32,
        "tool_selection_delta": (
            (run.retries + run.extra_ok) - (baseline.retries + baseline.extra_ok)
        )
        * Q32,
        "eval_score_delta": _q32_of_micros(
            round(run.score * 1_000_000) - round(baseline.score * 1_000_000)
        ),
        "cost_delta": (run.cost - baseline.cost) * Q32,
    }


async def _record_runs(traces: dict[str, ProbeRun]) -> None:
    """Commit each run's trace through ``DecisionLog`` as the deciding
    principal and append its priced usage through ``Usage``."""
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    decider = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    service = await MedullaClient.connect(
        socket_path=socket_path,
        verified_context=request_context(scopes=["usage:write"]),
    )
    try:
        for run_id, run in traces.items():
            stored = await dc.DecisionClient(client=decider).log(
                {
                    "op": "run_record",
                    "tenant_id": TEST_TENANT,
                    "record": {"kind": "trace", "trace": _trace(run_id, run)},
                }
            )
            assert stored["stored"]["kind"] == "trace", stored
        usage = dc.DecisionClient(client=service)
        price = {
            "schema_version": 1,
            "model_key": PROBE_MODEL,
            "valid_from_ms": 0,
            "in_per_mtok_micro": 1,
            "out_per_mtok_micro": 0,
            "cached_per_mtok_micro": 0,
        }
        await usage.usage(dc.usage_put_price_op({"entries": [price]}))
        now = int(time.time() * 1000)
        records = [
            dc.usage_record(
                TEST_TENANT,
                "contrast-probe",
                "probe",
                run_id,
                now,
                model_key=PROBE_MODEL,
                tokens_in=run.cost * 1_000_000,
                tokens_out=0,
            )
            for run_id, run in traces.items()
        ]
        appended = await usage.usage(dc.usage_append_op(records))
        assert appended["appended"] == len(records)
    finally:
        await decider.close()
        await service.close()


def seed_probe_runs(
    clean_graph: Any,
    key: str,
    versions: list[tuple[str, dict[str, Any]]],
    runs: list[ProbeRun],
) -> dict[str, str]:
    """Write the held versions and the measured evidence of ``runs``; answer
    each run's trace id by name.

    Trace ids carry a fresh tag: the run traces and the usage ledger are
    tenant-wide and outlive the test, so a reused id would replay old rows."""
    tag = uuid.uuid4().hex[:12]
    traces = {f"trace:{key}:{tag}:{run.name}": run for run in runs}
    asyncio.run(_record_runs(traces))
    nodes = list(versions)
    edges: list[tuple[str, str]] = []
    for offset, (trace_id, run) in enumerate(traces.items()):
        score_id = f"score:{trace_id}"
        nodes += [
            (probe_run_node_id(trace_id), probe_run_props(trace_id, 200 + offset)),
            (trace_id, {"node_type": "ExperimentRun"}),
            (
                score_id,
                {
                    "node_type": "Score",
                    "value": run.score,
                    "range_min": 0.0,
                    "range_max": 1.0,
                    "grader_kind": "human",
                },
            ),
        ]
        edges.append((score_id, trace_id))
    _apply_evidence(clean_graph, f"{key}:{tag}", nodes, edges)
    return {run.name: trace_id for trace_id, run in traces.items()}


def _apply_evidence(
    clean_graph: Any,
    key: str,
    nodes: list[tuple[str, dict[str, Any]]],
    scores: Sequence[tuple[str, str]] = (),
) -> None:
    """One envelope, in application order, writing the evidence nodes and then
    one ``scoresRun`` edge per ``(score, run)`` pair.

    The batch id derives from the caller's key: the shared session graph
    persists batch receipts across tests, so two tests may never claim the
    same batch id under different idempotency keys."""
    envelope = _envelope(
        _graph_name(clean_graph),
        key,
        key,
        1,
        [
            op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ]
        + [
            op(
                len(nodes) + offset,
                "AddEdge",
                {
                    "source_id": score_id,
                    "target_id": run_id,
                    "properties_msgpack": msgpack.packb({"relationship": "scoresRun"}),
                },
            )
            for offset, (score_id, run_id) in enumerate(scores)
        ],
    )
    results = clean_graph.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results


def contrast_schema_body() -> dict[str, Any]:
    """The pinned feature schema: exactly the four contrast deltas, declared
    in the sorted order the record's matrix names them."""
    return ds.feature_schema_body(
        [ds.feature(name, ds.number(name)) for name in CONTRAST_FEATURES]
    )


def candidate(version_ref: str, probe_trace_id: str) -> dc.DeclaredOption:
    """One declared candidate option: the version ref as option id and the
    probe trace as declared text. The deltas are the engine's to measure."""
    return dc.DeclaredOption(version_ref, texts={"probe_trace_id": probe_trace_id})


def _contrast_params(graph: str, baseline: str) -> list[dict[str, Any]]:
    return [
        dc.param("baseline_trace_id", "text", baseline),
        dc.param("conditions_digest", "text", CONDITIONS_DIGEST),
        dc.param("graph", "text", graph),
        dc.param("task_class", "iri", TASK_CLASS),
        dc.param("task_ref", "text", TASK_REF),
    ]


async def _contrast_journey(
    schema: dict[str, Any],
    options: list[dc.DeclaredOption],
    question_id: str,
    graph: str,
    baseline: str,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], dict[str, Any]]:
    """Decide, commit through the decision log and read the entry back."""
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    client = await MedullaClient.connect(
        socket_path=socket_path,
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        request = dc.decide_request(
            TEST_TENANT,
            (question_id, "harness_contrast", "ordinary"),
            dc.declared(options),
            schema,
            params=_contrast_params(graph, baseline),
        )
        payload = await decision.decide(request)
        record = payload["records"][0]
        committed = await decision.log(dc.commit_op(record))
        entry = await decision.log(dc.get_op(TEST_TENANT, record["record_id"]))
        return payload, record, committed, entry
    finally:
        await client.close()


def _expected_matrix(
    ordered_refs: list[str],
    traces: dict[str, str],
    deltas: dict[str, dict[str, int]],
) -> dict[str, Any]:
    """The inline matrix the engine seals: candidate order outer, the four
    delta features (sorted) inner, Q32."""
    values: list[int] = []
    for ref in ordered_refs:
        delta = deltas[traces[ref]]
        values.extend(
            [
                delta["cost_delta"],
                delta["eval_score_delta"],
                delta["retry_delta"],
                delta["tool_selection_delta"],
            ]
        )
    return {
        "matrix": "inline",
        "candidate_ids": ordered_refs,
        "feature_names": list(CONTRAST_FEATURES),
        "scale": "q32",
        "values": values,
    }


# spec: MEDULLA-DECISION-ENGINE-R803.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R803.3")
def test_contrast_decide_stores_record(clean_graph):
    # The evidence: the two held candidate versions (one per component),
    # the baseline and the two candidate probe runs under the identical
    # conditions -- each run's trace, score row and priced usage -- written
    # through the generated client write, DecisionLog and Usage.
    improving_ref = version_node_id(COMPONENT_A, "blob:improving")
    neutral_ref = version_node_id(COMPONENT_B, "blob:neutral")
    base = ProbeRun("baseline", retries=1, extra_ok=0, score=0.25, cost=3)
    better = ProbeRun("improving", retries=0, extra_ok=1, score=0.75, cost=2)
    same = ProbeRun("neutral", retries=1, extra_ok=0, score=0.25, cost=2)
    ids = seed_probe_runs(
        clean_graph,
        "h2-r803:evidence:adopt",
        [
            (improving_ref, version_props(COMPONENT_A, "blob:improving", 10, 100)),
            (neutral_ref, version_props(COMPONENT_B, "blob:neutral", 10, 101)),
        ],
        [base, better, same],
    )
    baseline, improving_trace, neutral_trace = (
        ids["baseline"],
        ids["improving"],
        ids["neutral"],
    )
    improving = measured_deltas(base, better)
    neutral = measured_deltas(base, same)
    assert improving == {
        "retry_delta": -Q32,
        "tool_selection_delta": 0,
        "eval_score_delta": Q32 // 2,
        "cost_delta": -Q32,
    }
    assert neutral == {
        "retry_delta": 0,
        "tool_selection_delta": 0,
        "eval_score_delta": 0,
        "cost_delta": -Q32,
    }

    schema = publish(
        clean_graph, "h2-r803-schema", "feature_schema", contrast_schema_body()
    )
    graph = _graph_name(clean_graph)
    options = [
        candidate(improving_ref, improving_trace),
        candidate(neutral_ref, neutral_trace),
    ]
    payload, record, committed, entry = asyncio.run(
        _contrast_journey(schema, options, "q:h2-r803:adopt", graph, baseline)
    )

    # The answer is one sealed record; the decision is stored in its inputs
    # with the adopted candidate and the per-candidate deltas pinned.
    ordered_refs = sorted([improving_ref, neutral_ref])
    traces = {improving_ref: improving_trace, neutral_ref: neutral_trace}
    deltas = {improving_trace: improving, neutral_trace: neutral}
    assert payload["schema_version"] == record["schema_version"]
    assert payload["inputs_digest"] == record["inputs_digest"]
    assert len(payload["records"]) == 1
    assert record["question"] == {
        "question_id": "q:h2-r803:adopt",
        "kind": "harness_contrast",
        "safety": "ordinary",
    }
    assert record["candidate_source"]["source"] == "declared"
    assert record["outcome"] == {
        "outcome": "explored",
        "option_id": improving_ref,
        "propensity": {"numerator": 1, "denominator": 1},
    }
    assert record["resolution_kind"] == "statistical"
    decision = record["inputs"]["harness_contrast"]
    assert decision["task_ref"] == TASK_REF
    assert decision["task_class"] == TASK_CLASS
    assert decision["candidate_version_refs"] == ordered_refs
    assert decision["conditions_digest"] == CONDITIONS_DIGEST
    assert decision["probe_trace_ids"] == [traces[ref] for ref in ordered_refs]
    assert decision["deltas"] == [
        {
            "task_ref": TASK_REF,
            "conditions_digest": CONDITIONS_DIGEST,
            "run_a_trace_id": baseline,
            "run_b_trace_id": traces[ref],
            **deltas[traces[ref]],
        }
        for ref in ordered_refs
    ]
    assert decision["adopted_version_ref"] == improving_ref
    assert decision.get("abstain_reason") is None
    assert record["inputs"]["feature_matrix"] == _expected_matrix(
        ordered_refs, traces, deltas
    )
    assert record["inputs"]["head"] is None
    assert "harness_evaluation" not in record["inputs"]
    assert record["inputs"]["policy"] == {"policy": "default"}

    # The record commits through the decision log (the commit runs the
    # contrast's verify-replay) and reads back under its record id.
    assert committed["record_id"] == record["record_id"]
    # A first commit is fresh: replayed flips to true only when an identical
    # commit has already landed under the same record id.
    assert committed["replayed"] is False
    assert committed["record_digest"] == record["record_digest"]
    assert entry["record"]["record_id"] == record["record_id"]
    assert entry["record"]["outcome"] == record["outcome"]
    assert entry["record"]["inputs"]["harness_contrast"] == decision
    assert entry["record"]["inputs"]["feature_matrix"] == _expected_matrix(
        ordered_refs, traces, deltas
    )
    assert entry["record"]["record_digest"] == record["record_digest"]
    assert entry["visibility"]["visibility"] == "principal"


# spec: MEDULLA-DECISION-ENGINE-R803.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R803.6")
def test_contrast_hot_path_served(clean_graph):
    # One improving and one neutral candidate adopt the improving one; two
    # non-improving candidates abstain with no_improvement and the
    # abstention stores through the same commit path.
    a_ref = version_node_id(COMPONENT_A, "blob:hot-a")
    b_ref = version_node_id(COMPONENT_B, "blob:hot-b")
    ids = seed_probe_runs(
        clean_graph,
        "h2-r803:evidence:hot",
        [
            (a_ref, version_props(COMPONENT_A, "blob:hot-a", 10, 100)),
            (b_ref, version_props(COMPONENT_B, "blob:hot-b", 10, 101)),
        ],
        [
            ProbeRun("baseline", retries=0, extra_ok=1, score=0.25, cost=3),
            # Improving: a higher independent score at a lower cost.
            ProbeRun("hot-a", retries=0, extra_ok=1, score=0.75, cost=2),
            # Neutral, flat: the baseline's score at a lower cost.
            ProbeRun("hot-b", retries=0, extra_ok=1, score=0.25, cost=2),
            ProbeRun("hot-c", retries=0, extra_ok=1, score=0.25, cost=2),
            # Worse: a retry, a lower score and a higher cost.
            ProbeRun("hot-d", retries=1, extra_ok=0, score=0.0, cost=4),
        ],
    )

    schema = publish(
        clean_graph, "h2-r803-hot-schema", "feature_schema", contrast_schema_body()
    )
    graph = _graph_name(clean_graph)

    improving = candidate(a_ref, ids["hot-a"])
    neutral = candidate(b_ref, ids["hot-b"])
    _, record, committed, _ = asyncio.run(
        _contrast_journey(
            schema, [improving, neutral], "q:h2-r803:hot-adopt", graph, ids["baseline"]
        )
    )
    assert record["outcome"] == {
        "outcome": "explored",
        "option_id": a_ref,
        "propensity": {"numerator": 1, "denominator": 1},
    }
    assert record["inputs"]["harness_contrast"]["abstain_reason"] is None
    # A first commit is fresh: replayed flips to true only when an identical
    # commit has already landed under the same record id.
    assert committed["replayed"] is False

    flat = candidate(a_ref, ids["hot-c"])
    worse = candidate(b_ref, ids["hot-d"])
    _, record, committed, entry = asyncio.run(
        _contrast_journey(
            schema, [flat, worse], "q:h2-r803:hot-abstain", graph, ids["baseline"]
        )
    )
    assert record["outcome"] == {
        "outcome": "abstained",
        "reasons": [{"reason": "insufficient_confidence"}],
    }
    assert record["resolution_kind"] == "abstention"
    decision = record["inputs"]["harness_contrast"]
    assert decision["abstain_reason"] == "no_improvement"
    assert decision.get("adopted_version_ref") is None
    # The abstention commits and reads back with its closed reason.
    assert committed["record_id"] == record["record_id"]
    # A first commit is fresh: replayed flips to true only when an identical
    # commit has already landed under the same record id.
    assert committed["replayed"] is False
    assert entry["record"]["inputs"]["harness_contrast"]["abstain_reason"] == (
        "no_improvement"
    )
