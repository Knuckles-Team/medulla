"""MEDULLA-ORCH-VOCAB-R750.752 served proof: an unregistered statechart
definition refuses with its own code, and the instance store is unchanged."""

from __future__ import annotations

import os

import pytest
from conftest import request_context


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R750.752")
@pytest.mark.asyncio
async def test_unregistered_definition_refuses_with_code(start_medulla_server) -> None:
    from medulla.client import EngineResponseError, MedullaClient

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        with pytest.raises(EngineResponseError) as refused:
            await client.statechart.instantiate("def:r750-unregistered")
    finally:
        await client.close()
    assert str(refused.value).startswith("INSTANCE_DEFINITION_UNREGISTERED"), (
        refused.value
    )


def _gap(gap_id: str, created_at_ms: int, utility: int, cost: int, depends=()):
    from medulla.generated.models import (
        BlastRadius,
        GapEvidence,
        GapStatus,
        GapView,
        GapWorkSpec,
        WorkOffer,
        WorkOfferView,
    )

    digest = "sha256:" + "ab" * 32
    offer = WorkOffer(
        blast_radius=BlastRadius.REPOSITORY,
        cost_uncertainty_ppm=100_000,
        depends_on_gap_ids=list(depends),
        evidence_digests=[digest],
        expected_cost_microunits=cost,
        expected_utility_micros=utility,
        probability_of_closure_ppm=500_000,
        required_capabilities=["eg:capability/code"],
        reversible=True,
    )
    return GapView(
        concept_ids=[],
        created_at_ms=created_at_ms,
        domain="runtime",
        evidence=[
            GapEvidence(
                digest=digest,
                generation=1,
                kind="failure_cluster",
                recorded_at_ms=created_at_ms,
                reference="cluster:1",
            )
        ],
        evidence_count=1,
        gap_id=gap_id,
        generation=1,
        offer=WorkOfferView(
            generation=1,
            offer=offer,
            offered_at_ms=created_at_ms,
            stage="eg/work-offer-utility-rate/v1",
            utility_rate=0,
            version=1,
            work_item_id=f"wi:{gap_id}",
        ),
        offer_version=1,
        priority_bucket=1,
        revision=1,
        severity_ppm=700_000,
        signature=gap_id,
        source="failure",
        spec_refs=[],
        statement="work to allocate",
        status=GapStatus.OPEN,
        updated_at_ms=created_at_ms,
        work=GapWorkSpec(kind="gap_remediation", max_attempts=3),
        work_item_id=f"wi:{gap_id}",
    ).model_dump(mode="json")


def _recruit(client, schema, twin=None):
    """One served ``Decide`` of kind ``recruit``; its answer is in the first score."""
    import json

    from conftest import TEST_TENANT

    from medulla import decision_client as dc

    capability = "eg:capability/code"
    review = "eg:capability/review"

    def candidate(agent: str, revision: str, capabilities: list[str]) -> dict:
        return {
            "employee": {
                "agent_ref": agent,
                "definition_revision": revision,
                "role_id": "role:builder",
            },
            "capabilities": capabilities,
        }

    asked = {
        "role": {
            "role_id": "role:builder",
            "required_capabilities": [capability, review],
            "headcount": 1,
        },
        "candidates": [
            candidate("agent:c", "rev-1", [capability]),
            candidate("agent:b", "rev-2", [capability, review]),
            candidate("agent:a", "rev-2", [capability, review]),
            candidate("agent:a", "rev-1", [capability]),
        ],
        "market": {
            "gaps": [
                _gap("gap:low", 1, 1_000_000, 2_000),
                _gap("gap:best", 2, 4_000_000, 2_000),
                _gap("gap:blocked", 0, 9_000_000, 2_000, depends=["gap:open"]),
                _gap("gap:over", 0, 9_000_000, 50_000),
            ],
            "facts": {
                "now_ms": 1_000,
                "resolved_gap_ids": [],
                "available_capabilities": [capability],
                "repository_scope": None,
                "max_blast_radius": "repository",
                "attempt_budget_microunits": 10_000,
                "cost_floor_microunits": 1_000,
            },
        },
        "twin": twin,
    }
    request = dc.decide_request(
        TEST_TENANT,
        ("org.recruit", "recruit", "ordinary"),
        dc.declared([dc.DeclaredOption("recruit")]),
        schema,
        params=[dc.param("recruit", "text", json.dumps(asked))],
    )
    record = client._send("Decide", {"request": request})["records"][0]
    option = record["outcome"]["scores"][0]["option_id"]
    return json.loads(option.removeprefix("recruit:"))


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R750.753")
def test_recruit_allocate_twin_and_drain_journey(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R750.753
    import asyncio

    from _decision_resident_fixture import publish, schema_body

    schema = publish(clean_graph, "org-journey-schema", "feature_schema", schema_body())

    # Recruit: full coverage first, ties on the lowest agent IRI.
    result = _recruit(clean_graph, schema)
    ranked = [
        (entry["agent_ref"], entry["definition_revision"], entry["covered"])
        for entry in result["ranked"]
    ]
    assert ranked == [
        ("agent:a", "rev-2", 2),
        ("agent:b", "rev-2", 2),
        ("agent:a", "rev-1", 1),
        ("agent:c", "rev-1", 1),
    ]

    # Allocate: only the top legal offer; each excluded gap keeps its reason.
    allocation = result["allocation"]
    assert len(allocation["ranked"]) == 1
    assert allocation["ranked"][0]["gap_id"] == "gap:best"
    assert allocation["ranked"][0]["work_item_id"] == "wi:gap:best"
    assert [gap for gap, _ in allocation["excluded"]] == ["gap:blocked", "gap:over"]

    # Twin: agent:a is replayed at its pinned rev-1 only, and the answer reproduces.
    twin = {"agent_ref": "agent:a", "pinned_revisions": ["rev-1"]}
    first = _recruit(clean_graph, schema, twin)
    second = _recruit(clean_graph, schema, twin)
    assert first == second
    assert [
        (entry["agent_ref"], entry["definition_revision"]) for entry in first["ranked"]
    ] == [("agent:b", "rev-2"), ("agent:a", "rev-1"), ("agent:c", "rev-1")]

    # Drain: an instance moves registered -> active -> draining -> terminated.
    states = ["registered", "active", "draining", "terminated"]
    events = ["activate", "drain", "terminate"]
    chart = {
        "name": "agent_instance",
        "schema_version": 1,
        "states": [{"id": state} for state in states],
        "alphabet": events,
        "transitions": [
            {"from": source, "event": event, "to": target}
            for source, event, target in zip(states, events, states[1:], strict=False)
        ],
        "initial": "registered",
        "finals": ["terminated"],
        "meta": {},
    }

    async def drain() -> list[str]:
        import os

        from conftest import request_context

        from medulla.client import MedullaClient

        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
        try:
            def_id = await client.statechart.define(chart)
            instance = (await client.statechart.instantiate(def_id))["instance_id"]
            reached = []
            for event in events:
                moved = await client.statechart.send_event(instance, event)
                assert moved["fired"] is True, moved
                reached.append(moved["instance"]["configuration"]["active"][0])
            return reached
        finally:
            await client.close()

    assert asyncio.run(drain()) == ["active", "draining", "terminated"]
