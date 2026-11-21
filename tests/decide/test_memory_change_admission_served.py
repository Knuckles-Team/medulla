"""MEDULLA-DECISION-ENGINE-R141.102 served memory-change evaluation: a rewrite
proposal and a benchmark are submitted to `DecisionEval`, bound to the stored
head, the pinned policy and the dataset, and a receipt never changes memory."""

from __future__ import annotations

import pytest
from _memory_receipts import (
    DOC,
    benchmark,
    pin_policy,
    proposal,
    read,
    submit,
    sync,
)

from medulla import decision_client as dc
from medulla.client import EngineResponseError

def test_real_decision_eval_binds_memory_candidate_and_metrics(clean_graph):
    client = clean_graph
    policy_ref, digest = pin_policy(client)
    sync(client, "req-1", None, "first")
    sync(client, "req-2", 1, "second")
    before = read(client)

    admit = submit(
        client, dc.memory_rewrite_op(proposal(policy_ref, digest, "rw-1", 20))
    )
    assert admit["state"]["state"] == "succeeded", admit["state"]
    evaluation = admit["state"]["output"]["evaluation"]
    assert evaluation["verdict"] == "admit"
    assert evaluation["fidelity_q4"] == 10000
    assert evaluation["proposal"]["policy"] == digest

    discard = submit(
        client, dc.memory_rewrite_op(proposal(policy_ref, digest, "rw-2", 5))
    )
    out = discard["state"]["output"]["evaluation"]
    assert out["verdict"] == "discard"
    assert "FIDELITY_BELOW_MIN" in out["reason_codes"]

    stale = proposal(policy_ref, digest, "rw-3", 20)
    stale["active"]["revision"] = 1
    refused = submit(client, dc.memory_rewrite_op(stale))
    assert refused["state"]["state"] == "failed"
    assert "MEMORY_REVISION_STALE" in refused["state"]["code"]

    wrong_policy = submit(
        client, dc.memory_rewrite_op(proposal(policy_ref, "f" * 64, "rw-4", 20))
    )
    assert wrong_policy["state"]["state"] == "failed"

    good = submit(client, dc.memory_benchmark_op(benchmark(policy_ref, "bm-1", True)))
    receipt = good["state"]["output"]["receipt"]
    assert receipt["verdict"] == "admit"
    assert receipt["candidate_metrics"]["queries"] == 4
    assert receipt["policy_digest"] == digest
    assert any(p.startswith("memory:dataset=") for p in receipt["premises"])

    bad = submit(client, dc.memory_benchmark_op(benchmark(policy_ref, "bm-2", False)))
    receipt = bad["state"]["output"]["receipt"]
    assert receipt["verdict"] == "discard"
    assert receipt["reason_codes"][0] == "RECALL_BELOW_MIN"

    # A reused key with a different request is refused at the door.
    with pytest.raises(EngineResponseError, match="IDEMPOTENCY_CONFLICT"):
        submit(client, dc.memory_benchmark_op(benchmark(policy_ref, "bm-1", False)))

    assert read(client) == before
