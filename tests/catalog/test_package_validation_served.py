"""Skill package publication over the served wire (MEDULLA-DECISION-ENGINE-R160.6).

Package runs are recorded with ``DecisionLog`` ``learn`` (write ``package_run``),
then ``PublishSkillPackage`` publishes a valid package (two passing runs for its
digest) and refuses an invalid one (one passing and one failing run, below the
minimum pass rate) with ``PACKAGE_VALIDATION_FAILED``.
"""

from __future__ import annotations

import hashlib
from typing import Any

import pytest
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla.client import EngineResponseError


def _digest(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _record_run(client: Any, run_id: str, content: bytes, passed: bool) -> None:
    client._send(
        "DecisionLog",
        {
            "op": dc.learn_op(
                TEST_TENANT,
                "package_run",
                run={
                    "run_id": run_id,
                    "package_digest": _digest(content),
                    "passed": passed,
                },
            )
        },
    )


def _publish(client: Any, package_id: str, content: bytes) -> dict[str, Any]:
    return client._send(
        "PublishSkillPackage",
        {
            "request": {
                "package_id": package_id,
                "package": {
                    "schema_version": 1,
                    "uri": "skill://triage",
                    "digest": _digest(content),
                    "ontology_refs": [],
                    "graph_revision": 1,
                },
            }
        },
    )


# spec: MEDULLA-DECISION-ENGINE-R160.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R160.6")
def test_invalid_packages_refuse_publication(clean_graph):
    client = clean_graph
    valid = b"valid package"
    _record_run(client, "run-valid-a", valid, True)
    _record_run(client, "run-valid-b", valid, True)
    invalid = b"invalid package"
    _record_run(client, "run-invalid-a", invalid, True)
    _record_run(client, "run-invalid-b", invalid, False)

    published = _publish(client, "package-valid", valid)
    receipt = published.get("receipt", published)
    assert receipt["package_id"] == "package-valid"
    assert receipt["validation_result"]["validation_passed"] is True
    assert receipt["validation_result"]["rendered_digest"] == _digest(valid)

    with pytest.raises(EngineResponseError, match="PACKAGE_VALIDATION_FAILED"):
        _publish(client, "package-invalid", invalid)
