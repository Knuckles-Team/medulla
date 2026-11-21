"""MEDULLA-DECISION-ENGINE-R129.6 served promotion over the wire client.

The entry point is the served ``DecisionLog`` method, op ``promote_rule``. A logged abstention
is resolved five times by the verified human caller. A rule that violates a stored invariant is
refused ``RULE_CONTRADICTS_INVARIANT`` and activates nothing; a compatible rule with the same
five resolutions promotes and appears in ``GuardrailEntailment``.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from conftest import TEST_TENANT
from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)
from test_canonical_body_served import _library_context, _skill_draft

from medulla import decision_client as dc

TASK_CLASS = "eg:task"


def _publish_invariant(client: Any, iri: str, forbidden: str) -> None:
    component = _skill_draft(iri)
    component.update(
        kind="rule_invariant",
        summary="rule promotion invariant",
        attributes={"forbidden_rule_id": forbidden, "on_class": TASK_CLASS},
    )
    client._send(
        "AgentComponent",
        {
            "op": {
                "op": "publish",
                "request": {
                    "context": _library_context(0),
                    "component": component,
                    "evaluation_receipt_digest": None,
                },
            }
        },
    )


def _logged_abstention(client: Any, suffix: str) -> str:
    pin = publish(client, f"promotion-schema-{suffix}", "feature_schema", schema_body())
    asked = dc.decide_request(
        TEST_TENANT,
        ("au.retrieval-plan", "retrieval_plan", "ordinary"),
        dc.declared(
            [
                dc.DeclaredOption("plan-deep", numbers={"score": 1 << 31}),
                dc.DeclaredOption("plan-hyde", numbers={"score": 1 << 32}),
            ]
        ),
        pin,
    )
    record = client._send("Decide", {"request": asked})["records"][0]
    assert record["outcome"]["outcome"] == "abstained", record["outcome"]
    return client._send("DecisionLog", {"op": dc.commit_op(record)})["record_id"]


# spec: MEDULLA-DECISION-ENGINE-R129.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R129.6")
def test_invariant_blocks_promotion(authenticated_plan_client):  # noqa: F811
    client = authenticated_plan_client
    suffix = uuid.uuid4().hex[:10]
    bad, good = f"rule:bad:{suffix}", f"rule:good:{suffix}"
    try:
        record_id = _logged_abstention(client, suffix)
        ids = [f"human-{suffix}-{i}" for i in range(5)]
        for resolution_id in ids:
            client._send(
                "DecisionLog",
                {
                    "op": dc.resolve_op(
                        TEST_TENANT, record_id, resolution_id, "plan-deep"
                    )
                },
            )
        _publish_invariant(client, f"inv:{suffix}", bad)

        def rule(rule_id: str) -> dict[str, str]:
            return {
                "rule_id": rule_id,
                "applies_to_class": TASK_CLASS,
                "source_generation": f"gen-{suffix}",
            }

        def entailed() -> set[str]:
            answer = client._send(
                "GuardrailEntailment",
                {"request": {"tenant_id": TEST_TENANT, "task_iri": TASK_CLASS}},
            )
            return {row["rule_id"] for row in answer["rules"]}

        # The rule that violates the stored invariant is refused and activates nothing.
        with pytest.raises(RuntimeError, match="RULE_CONTRADICTS_INVARIANT"):
            client._send(
                "DecisionLog",
                {"op": dc.promote_rule_op(TEST_TENANT, rule(bad), ids)},
            )
        assert bad not in entailed()

        # A compatible rule with the same resolutions promotes and is entailed.
        promoted = client._send(
            "DecisionLog",
            {"op": dc.promote_rule_op(TEST_TENANT, rule(good), ids)},
        )
        assert promoted["published"] is True, promoted
        assert good in entailed()
    finally:
        client.close()
