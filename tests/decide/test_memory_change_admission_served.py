"""MEDULLA-DECISION-ENGINE-R141.102 served memory-change evaluation: a rewrite
proposal and a benchmark are submitted to `DecisionEval`, bound to the stored
head, the pinned policy and the dataset, and a receipt never changes memory."""

from __future__ import annotations

import hashlib
import json
from typing import Any

from _decision_resident_fixture import publication
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla.decision import policy_digest
from medulla.decision._digest import raw_digest

DOC = "memory:served-1"
REWRITE_DOMAIN = "eg/memory-rewrite/v1"


def _policy_body() -> dict[str, Any]:
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
        "cold_start": {"mode": "advisory_uncalibrated"},
        "statistical": None,
    }


def _pin_policy(client: Any) -> tuple[dict[str, Any], str]:
    body = _policy_body()
    request = publication("memory-policy", "decision_policy", {})
    component = request["op"]["request"]["component"]
    component["content_digest"] = policy_digest(body)
    component["attributes"] = {"decision.policy": json.dumps(body)}
    entry = client._send("AgentComponent", request)["component"]
    pinned = {
        "component_id": "memory-policy",
        "kind": "decision_policy",
        "definition_digest": entry["definition_digest"],
    }
    return {"policy": "pinned", "component": pinned}, policy_digest(body).removeprefix(
        "sha256:"
    )


def _sync(
    client: Any, request_id: str, expected: int | None, content: str
) -> dict[str, Any]:
    return client._send(
        "SyncMemorySnapshot",
        {
            "request": {
                "request_id": request_id,
                "doc_id": DOC,
                "action": {
                    "action": "submit",
                    "expected_revision": expected,
                    "content": content,
                    "content_digest": hashlib.sha256(content.encode()).hexdigest(),
                },
            }
        },
    )


def _read(client: Any) -> dict[str, Any]:
    return client._send("ReadMemorySnapshot", {"doc_id": DOC, "revision": None})


def _signals(count: int) -> list[str]:
    return [f"signal number {i}" for i in range(count)]


def _proposal(
    policy_ref: dict[str, Any], digest: str, key: str, kept: int
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "tenant_id": TEST_TENANT,
        "idempotency_key": key,
        "graph": "default",
        "record_id": DOC,
        "active": {"revision": 2, "digest": "a" * 64},
        "candidate": {"revision": 3, "digest": "b" * 64},
        "active_signals": _signals(20),
        "candidate_signals": _signals(kept),
        "token_count": 40,
        "metric_id": "memory_fidelity_set_overlap",
        "policy": digest,
        "policy_ref": policy_ref,
        "history": {"entries": [], "history_digest": raw_digest(REWRITE_DOMAIN, [])},
    }


def _answer(c: str) -> str:
    return hashlib.sha256(c.encode()).hexdigest()


def _benchmark(policy_ref: dict[str, Any], key: str, good: bool) -> dict[str, Any]:
    queries = [
        {
            "query_id": f"q{i}",
            "expected_evidence_ids": [f"e{i}"],
            "reference_answer_digest": _answer(f"answer {i}"),
        }
        for i in range(4)
    ]

    def run(revision: dict[str, Any], hit: bool) -> dict[str, Any]:
        return {
            "revision": revision,
            "results": [
                {
                    "query_id": q["query_id"],
                    "top_ids": q["expected_evidence_ids"] if hit else ["none"],
                    "answer_text": f"answer {i}" if hit else "wrong",
                }
                for i, q in enumerate(queries)
            ],
        }

    base = {"revision": 2, "digest": "a" * 64}
    cand = {"revision": 3, "digest": "b" * 64}
    return {
        "schema_version": 1,
        "tenant_id": TEST_TENANT,
        "idempotency_key": key,
        "record_id": DOC,
        "change_kind": "rewrite",
        "baseline": base,
        "candidate": cand,
        "dataset": {"id": "ds", "revision": 1, "digest": "d" * 64, "queries": queries},
        "k": 3,
        "evaluator_revision": "eval-1",
        "policy": policy_ref,
        "baseline_results": run(base, True),
        "candidate_results": run(cand, good),
    }


def _submit(client: Any, op: dict[str, Any]) -> dict[str, Any]:
    return client._send("DecisionEval", op)


def test_real_decision_eval_binds_memory_candidate_and_metrics(clean_graph):
    client = clean_graph
    policy_ref, digest = _pin_policy(client)
    _sync(client, "req-1", None, "first")
    _sync(client, "req-2", 1, "second")
    before = _read(client)

    admit = _submit(
        client, dc.memory_rewrite_op(_proposal(policy_ref, digest, "rw-1", 20))
    )
    assert admit["state"]["state"] == "succeeded", admit
    evaluation = admit["state"]["output"]["evaluation"]
    assert evaluation["verdict"] == "admit"
    assert evaluation["fidelity_q4"] == 10000
    assert evaluation["proposal"]["policy"] == digest

    discard = _submit(
        client, dc.memory_rewrite_op(_proposal(policy_ref, digest, "rw-2", 5))
    )
    out = discard["state"]["output"]["evaluation"]
    assert out["verdict"] == "discard"
    assert "FIDELITY_BELOW_MIN" in out["reason_codes"]

    stale = _proposal(policy_ref, digest, "rw-3", 20)
    stale["active"]["revision"] = 1
    refused = _submit(client, dc.memory_rewrite_op(stale))
    assert refused["state"]["state"] == "failed"
    assert "MEMORY_REVISION_STALE" in refused["state"]["code"]

    wrong_policy = _submit(
        client, dc.memory_rewrite_op(_proposal(policy_ref, "f" * 64, "rw-4", 20))
    )
    assert wrong_policy["state"]["state"] == "failed"

    good = _submit(client, dc.memory_benchmark_op(_benchmark(policy_ref, "bm-1", True)))
    receipt = good["state"]["output"]["receipt"]
    assert receipt["verdict"] == "admit"
    assert receipt["candidate_metrics"]["queries"] == 4
    assert receipt["policy_digest"] == digest
    assert any(p.startswith("memory:dataset=") for p in receipt["premises"])

    bad = _submit(client, dc.memory_benchmark_op(_benchmark(policy_ref, "bm-2", False)))
    receipt = bad["state"]["output"]["receipt"]
    assert receipt["verdict"] == "discard"
    assert receipt["reason_codes"][0] == "RECALL_BELOW_MIN"

    conflict = _submit(
        client, dc.memory_benchmark_op(_benchmark(policy_ref, "bm-1", False))
    )
    assert "IDEMPOTENCY_CONFLICT" in str(conflict)

    assert _read(client) == before
