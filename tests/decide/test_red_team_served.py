"""MEDULLA-DECISION-ENGINE-R138.6 served receipt: a probe verdict over the wire.

The entry point is the served ``Decide`` method with the ``probe_verdict``
question kind. The caller publishes one attack, one transform and two scorers
as library components, declares one option per scorer result (option id = the
scorer id, ``score_permille`` as a declared Q32 fraction of 1.0) and names the
attack and transforms as typed params. The engine selects the scorers of the
attack's category, judges the attack and seals one advisory record whose
chosen label scores 1.0.
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import pytest
from _decision_resident_fixture import publication, schema_body
from _decision_resident_fixture import publish as publish_body
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

Q32 = 1 << 32


def _publish_red_team(
    client: Any, component_id: str, kind: str, attributes: dict[str, str]
) -> None:
    """Publish one red-team component with its typed attributes."""
    request = publication(component_id, kind, {"_": component_id})
    request["op"]["request"]["component"]["attributes"] = attributes
    client._send("AgentComponent", request)


def _result(scorer: str, permille: int) -> dc.DeclaredOption:
    return dc.DeclaredOption(
        scorer, numbers={"score_permille": (permille * Q32 + 500) // 1000}
    )


def _request(schema: dict[str, Any], results: list[dc.DeclaredOption]) -> dict:
    return dc.decide_request(
        TEST_TENANT,
        ("q:probe:served", "probe_verdict", "ordinary"),
        dc.declared(results),
        schema,
        params=[
            dc.param("attack_id", "iri", "attack-1"),
            dc.param("transform_ids", "iri_list", ["tx-1"]),
        ],
    )


async def _decide(request: dict[str, Any]) -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        return await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()


def _chosen(record: dict[str, Any]) -> str:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory"
    scores = outcome["scores"]
    assert [s["option_id"] for s in scores] == [
        "resisted",
        "compromised",
        "inconclusive",
    ]
    one = [s["option_id"] for s in scores if s["score"]["value"] == Q32]
    assert len(one) == 1
    assert all(s["score"]["value"] in (0, Q32) for s in scores)
    return one[0]


# spec: MEDULLA-DECISION-ENGINE-R138.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R138.6")
def test_probe_verdict_end_to_end(clean_graph):
    schema = publish_body(clean_graph, "probe-schema", "feature_schema", schema_body())
    _publish_red_team(
        clean_graph,
        "attack-1",
        "red_team_attack",
        {"category": "injection", "goal": "leak"},
    )
    _publish_red_team(clean_graph, "tx-1", "red_team_transform", {"kind": "base64"})
    for scorer, threshold in [("scorer-a", "700"), ("scorer-b", "500")]:
        _publish_red_team(
            clean_graph,
            scorer,
            "red_team_scorer",
            {"category": "injection", "threshold": threshold},
        )

    # A scorer clearing its threshold compromises the attack.
    payload = asyncio.run(
        _decide(_request(schema, [_result("scorer-a", 800), _result("scorer-b", 100)]))
    )
    record = payload["records"][0]
    assert _chosen(record) == "compromised"
    assert record["resolution_kind"] == "constraint"
    assert record["evidence_class"] == "claim"
    facts = {p["fact"] for p in record["premises"]}
    assert "scorer:scorer-a;score_permille=800;threshold_permille=700" in facts
    assert "scorer:scorer-b;score_permille=100;threshold_permille=500" in facts
    assert "attack:attack-1" in facts
    assert "transform:tx-1" in facts

    # Every scorer failing resists it.
    payload = asyncio.run(
        _decide(_request(schema, [_result("scorer-a", 100), _result("scorer-b", 100)]))
    )
    assert _chosen(payload["records"][0]) == "resisted"
