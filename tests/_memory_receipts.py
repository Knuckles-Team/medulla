"""The admit-receipt helpers the memory served tests share
(MEDULLA-DECISION-ENGINE-R141.102, MEDULLA-DECISION-ENGINE-R141.104,
MEDULLA-DECISION-ENGINE-R651.7): pin a decision policy, sync a snapshot, and
submit a rewrite proposal or a benchmark to `DecisionEval` to obtain the
receipt a gated memory change names."""

from __future__ import annotations

import hashlib
import json
from typing import Any

from _decision_resident_fixture import publication
from conftest import TEST_TENANT

from medulla.decision import policy_digest
from medulla.decision._digest import raw_digest

DOC = "memory:served-1"
REWRITE_DOMAIN = "eg/memory-rewrite/v1"


def policy_body() -> dict[str, Any]:
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


def pin_policy(client: Any) -> tuple[dict[str, Any], str]:
    body = policy_body()
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


def sync(
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


def read(client: Any) -> dict[str, Any]:
    return client._send("ReadMemorySnapshot", {"doc_id": DOC, "revision": None})


def signals(count: int) -> list[str]:
    return [f"signal number {i}" for i in range(count)]


def proposal(
    policy_ref: dict[str, Any], digest: str, key: str, kept: int
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "tenant_id": TEST_TENANT,
        "idempotency_key": key,
        "graph": "__commons__",  # the connection graph the snapshots are synced to
        "record_id": DOC,
        "active": {"revision": 2, "digest": "a" * 64},
        "candidate": {"revision": 3, "digest": "b" * 64},
        "active_signals": signals(20),
        "candidate_signals": signals(kept),
        "token_count": 40,
        "metric_id": "memory_fidelity_set_overlap",
        "policy": digest,
        "policy_ref": policy_ref,
        "history": {"entries": [], "history_digest": raw_digest(REWRITE_DOMAIN, [])},
    }


def answer(c: str) -> str:
    return hashlib.sha256(c.encode()).hexdigest()


def benchmark(
    policy_ref: dict[str, Any],
    key: str,
    good: bool,
    *,
    record_id: str = DOC,
    change_kind: str = "rewrite",
    baseline: dict[str, Any] | None = None,
    candidate: dict[str, Any] | None = None,
) -> dict[str, Any]:
    queries = [
        {
            "query_id": f"q{i}",
            "expected_evidence_ids": [f"e{i}"],
            "reference_answer_digest": answer(f"answer {i}"),
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

    base = baseline or {"revision": 2, "digest": "a" * 64}
    cand = candidate or {"revision": 3, "digest": "b" * 64}
    return {
        "schema_version": 1,
        "tenant_id": TEST_TENANT,
        "idempotency_key": key,
        "record_id": record_id,
        "change_kind": change_kind,
        "baseline": base,
        "candidate": cand,
        "dataset": {"id": "ds", "revision": 1, "digest": "d" * 64, "queries": queries},
        "k": 3,
        "evaluator_revision": "eval-1",
        "policy": policy_ref,
        "baseline_results": run(base, True),
        "candidate_results": run(cand, good),
    }


def submit(client: Any, op: dict[str, Any]) -> dict[str, Any]:
    return client._send("DecisionEval", {"op": op})
