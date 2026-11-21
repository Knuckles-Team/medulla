"""R010.2 L4: the Python client ``order_limit_check`` verdict equals the
``order_stage`` refusal for the same proposal, through the served engine."""

from __future__ import annotations

import hashlib
import json
from typing import Any

import pytest
from conftest import TEST_TENANT

SCHEMA_VERSION = "finance-v1"
RISK_LIMITS = [
    {"bound": 1000, "id": "qty", "metric": "position_size", "scope": "AAPL"},
    {
        "bound": 500000,
        "id": "notional",
        "metric": "position_size",
        "scope": "AAPL:notional",
    },
]
LEVERAGE_LIMITS = [
    {
        "approvers": ["alice"],
        "cfd_unavailable_us_retail": True,
        "instrument_id": "AAPL",
    }
]


def _framed(domain: str, body: bytes) -> str:
    """The engine's ``digest::of_json``: length-framed domain and body under SHA-256."""
    hasher = hashlib.sha256()
    for part in (domain.encode(), body):
        hasher.update(len(part).to_bytes(8, "big"))
        hasher.update(part)
    return "sha256:" + hasher.hexdigest()


def _policy_digest() -> str:
    policy = {
        "tenant_id": TEST_TENANT,
        "revision": 3,
        "leverage_limits": LEVERAGE_LIMITS,
        "risk_limits": RISK_LIMITS,
        "designated_approvers": ["alice"],
    }
    body = json.dumps(policy, separators=(",", ":")).encode()
    return _framed("finance.trading_policy.v1", body)


def _snapshot(quantity: int, notional: int) -> dict[str, Any]:
    payload = json.dumps(
        {"instrument_id": "AAPL", "signed_quantity": quantity, "notional": notional},
        separators=(",", ":"),
    ).encode()
    return {
        "tenant_id": TEST_TENANT,
        "proposal_id": "py-prop",
        "revision": 1,
        "predecessor_digest": None,
        "decision_id": "dec1",
        "decision_revision": 1,
        "decision_digest": "sha256:dec",
        "payload_schema_id": "finance.order",
        "payload_schema_revision": "1",
        "canonical_payload_bytes": list(payload),
        "payload_sha256": "sha256:" + hashlib.sha256(payload).hexdigest(),
        "policy_id": "pol1",
        "policy_revision": 3,
        "policy_digest": _policy_digest(),
        "evidence_refs": [
            {
                "owner_requirement_id": "MEDULLA-DECISION-ENGINE-R019",
                "record_id": "finance:TradingPolicy:pol1",
                "revision": 1,
                "digest": "sha256:ev",
            }
        ],
    }


def _terms(value: Any) -> str:
    return '"' + json.dumps(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


# spec: MEDULLA-FINANCE-PRIMITIVES-R010.2
def test_client_method_matches_stage_refusal(clean_graph) -> None:
    clean_graph.finance.record_put(
        "TradingPolicy",
        SCHEMA_VERSION,
        {
            "record_id": "pol1",
            "tenant": TEST_TENANT,
            "fields": {
                "tradingPolicyRevision": '"3"',
                "tradingPolicyLeverageLimits": _terms(LEVERAGE_LIMITS),
                "tradingPolicyRiskLimits": _terms(RISK_LIMITS),
                "tradingPolicyApprovers": _terms(["alice"]),
            },
        },
    )
    breaching = _snapshot(-1500, 900000)
    verdict = clean_graph.finance.order_limit_check(breaching)
    assert verdict["outcome"] == "Abstained"
    assert [(r["rule_id"], r["observed"], r["bound"]) for r in verdict["reasons"]] == [
        ("qty", 1500, 1000),
        ("notional", 900000, 500000),
    ]
    with pytest.raises(Exception, match="ORDER_LIMIT_BREACH") as refusal:
        clean_graph.finance.order_stage(breaching)
    for reason in verdict["reasons"]:
        assert reason["rule_id"] in str(refusal.value)
    assert clean_graph.finance.order_limit_check(_snapshot(10, 2000))["outcome"] == "Passed"
