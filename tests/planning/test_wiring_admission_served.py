"""Wiring admission of a plan through the served ``Decide`` (MEDULLA-DECISION-ENGINE-R171.6).

The client ingests a fixture repository (one manifest and two source files) into
the graph, then submits a wired plan, a plan with one unwired feature and a plan
with a stale architecture view digest. The wired plan is admitted, the unwired
feature abstains ``ADMISSION_FEATURE_UNWIRED`` and the stale digest refuses
``ADMISSION_STALE_VIEW``. No method is added: every call is ``Decide``.
"""

from __future__ import annotations

import hashlib
import re
from typing import Any

import msgpack
import pytest
from _decision_resident_fixture import publish
from _harness_envelope import draft_envelope as _envelope
from _harness_envelope import graph_name as _graph_name
from _harness_envelope import op as _op
from conftest import TEST_TENANT

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError

REPO = "repo:orders"
DIR = "services/orders"
GAP = hashlib.sha256(b"gap-a").hexdigest()


def _sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def _seed_repository(client: Any) -> None:
    nodes: list[tuple[str, dict[str, Any]]] = [
        (
            f"manifest:{REPO}",
            {
                "type": "FILE",
                "file": f"{DIR}/Cargo.toml",
                "repository_id": REPO,
                "tenant_id": TEST_TENANT,
            },
        )
    ]
    for name in ("main.rs", "lib.rs"):
        nodes.append(
            (
                f"symbol:{REPO}:{name}",
                {
                    "type": "SYMBOL",
                    "file": f"{DIR}/src/{name}",
                    "repository_id": REPO,
                    "tenant_id": TEST_TENANT,
                },
            )
        )
    envelope = _envelope(
        _graph_name(client),
        "w171:repo",
        "w171:repo",
        1,
        [
            _op(
                ordinal,
                "AddNode",
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
            )
            for ordinal, (node_id, props) in enumerate(nodes)
        ],
    )
    results = client.changes.apply_batch([envelope])
    assert [result["result"] for result in results] == ["applied"], results


def _feature(feature_id: str, container: str, file: str) -> dict[str, Any]:
    return {
        "feature_id": feature_id,
        "description_digest": _sha(feature_id),
        "container": container,
        "attach_point": {"file": file, "symbol": "run", "operation": "call"},
        "gap_digests": [GAP],
    }


def _decide(
    client: Any, schema: dict[str, Any], view_digest: str, features: list[dict]
) -> dict[str, Any]:
    plan = {
        "repository_id": REPO,
        "repository_revision": "rev-1",
        "architecture_view_digest": view_digest,
        "gap_proposal_digests": [GAP],
        "features": features,
    }
    request = dc.decide_request(
        TEST_TENANT,
        ("plan.admission", "planning_admission", "ordinary"),
        dc.declared([dc.DeclaredOption("admit_plan")]),
        schema,
        params=[
            dc.param("planning_admission", "planning_admission", plan),
            dc.param("graph", "iri", _graph_name(client)),
        ],
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


# spec: MEDULLA-DECISION-ENGINE-R171.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R171.6")
def test_unwired_feature_refused(clean_graph):
    client = clean_graph
    _seed_repository(client)
    schema = publish(
        client,
        "schema-route",
        "feature_schema",
        ds.feature_schema_body([ds.feature("signal", ds.number("signal"))]),
    )
    wired = [_feature("f1", "orders", f"{DIR}/src/main.rs")]

    # A stale view digest refuses and names the digest the repository derives now.
    with pytest.raises(EngineResponseError, match="ADMISSION_STALE_VIEW") as stale:
        _decide(client, schema, _sha("stale view"), wired)
    current = re.search(r"derives ([0-9a-f]{64})", str(stale.value))
    assert current, str(stale.value)
    view_digest = current.group(1)

    # A wired plan is admitted as a constraint over observed evidence.
    admitted = _decide(client, schema, view_digest, wired)
    assert admitted["outcome"]["outcome"] == "advisory", admitted["outcome"]
    assert admitted["outcome"]["scores"][0]["option_id"] == "admit_plan"
    assert admitted["resolution_kind"] == "constraint"
    assert admitted["evidence_class"] == "observation"

    # A feature attached to a file its container does not hold is held, named.
    held = _decide(
        client,
        schema,
        view_digest,
        [_feature("f-file", "orders", f"{DIR}/src/ghost.rs")],
    )
    assert held["outcome"]["outcome"] == "abstained", held["outcome"]
    reason = held["outcome"]["reasons"][0]
    assert reason["code"] == "ADMISSION_FEATURE_UNWIRED"
    assert "f-file" in reason["detail"]
