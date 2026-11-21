"""MEDULLA-DECISION-ENGINE-R142.102 served local-skill certification.

The entry point is served ``DecisionEval`` with the ``certify_local_skill`` op. A matching
atomic receipt is attested and its attestation, with its digest, is read from the job output. A
workflow that needs the attestation is then certified; the same workflow over another model
pin, after the pinned inputs changed, is refused ``CERT_DEPENDENCY_STALE``, and one whose
dependency was never attested is refused ``CERT_DEPENDENCY_MISSING``.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish
from _memory_receipts import submit
from conftest import TEST_TENANT
from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)

from medulla import decision_client as dc


def _pins(model_digest: str) -> dict[str, Any]:
    return {
        "model": {
            "component_id": "model-a",
            "kind": "model_profile",
            "definition_digest": model_digest,
        },
        "dataset": {"id": "dataset-a", "revision": 1, "digest": "c" * 64},
        "criteria_policy": {
            "component_id": "criteria-a",
            "kind": "decision_policy",
            "definition_digest": "sha256:cc",
        },
    }


def _request(
    key: str,
    skill: dict[str, Any],
    dependencies: list[dict[str, Any]],
    pins: dict[str, Any],
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "tenant_id": TEST_TENANT,
        "idempotency_key": key,
        "skill": skill,
        "classification": "workflow" if dependencies else "atomic",
        "dependencies": dependencies,
        "pins": pins,
        "criterion_ids": ["accuracy"],
        "receipt": {
            "run_id": f"run-{key}",
            "skill": skill,
            "pins": pins,
            "results": [
                {
                    "criterion_id": "accuracy",
                    "passed": True,
                    "evidence_digest": "e" * 64,
                }
            ],
            "completion_revision": 1,
        },
    }


def _certify(client: Any, request: dict[str, Any]) -> dict[str, Any]:
    return submit(client, dc.certify_local_skill_op(request))["state"]


def _code(state: dict[str, Any]) -> str:
    assert state["state"] == "failed", state
    return state["code"]


# spec: MEDULLA-DECISION-ENGINE-R142.102
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R142.102")
def test_certification_journey(authenticated_plan_client):  # noqa: F811
    client = authenticated_plan_client
    suffix = uuid.uuid4().hex[:10]
    try:
        atomic = publish(client, f"skill-atomic-{suffix}", "skill", {})
        other = publish(client, f"skill-other-{suffix}", "skill", {})
        workflow = {
            "component_id": f"skill-workflow-{suffix}",
            "kind": "skill",
            "definition_digest": "sha256:dd",
        }

        # A workflow before its dependency is attested is refused.
        early = _certify(
            client, _request(f"wf-0-{suffix}", workflow, [atomic], _pins("sha256:aa"))
        )
        assert _code(early).startswith("CERT_DEPENDENCY_MISSING")

        # The matching atomic receipt is attested and the attestation is read by its digest.
        state = _certify(
            client, _request(f"atomic-{suffix}", atomic, [], _pins("sha256:aa"))
        )
        assert state["state"] == "succeeded", state
        attestation = state["output"]["attestation"]
        assert attestation["digest"]
        assert "criterion:accuracy" in attestation["premises"]
        assert any(
            p.startswith(f"skill:skill-atomic-{suffix}@")
            for p in attestation["premises"]
        )

        # The workflow now passes over the same pins ...
        ok = _certify(
            client, _request(f"wf-1-{suffix}", workflow, [atomic], _pins("sha256:aa"))
        )
        assert ok["state"] == "succeeded", ok

        # ... changing the pinned model makes the attestation stale, and a dependency that was
        # never attested is missing.
        stale = _certify(
            client, _request(f"wf-2-{suffix}", workflow, [atomic], _pins("sha256:bb"))
        )
        assert _code(stale).startswith("CERT_DEPENDENCY_STALE")
        missing = _certify(
            client, _request(f"wf-3-{suffix}", workflow, [other], _pins("sha256:aa"))
        )
        assert _code(missing).startswith("CERT_DEPENDENCY_MISSING")
    finally:
        client.close()
