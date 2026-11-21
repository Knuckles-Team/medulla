"""Served standards drift (MEDULLA-UNIFIED-DATA-PLANE-R701.3).

`StandardsDrift` reads the verified tenant's `Standard` and `StandardsAsset` nodes, scores
every asset against its standard, groups the assets into capability cohorts and proposes
consolidations. A recommendation is only a proposal: the call writes nothing, and another
tenant's nodes are never read.
"""

from __future__ import annotations

import hashlib
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT

from medulla.client import EngineResponseError

OTHER_TENANT = "tenant:standards-other"


def carrier_scope(tenant: str) -> str:
    """The opaque carrier tenant scope the engine stamps for a verified tenant."""
    digest = hashlib.sha256(b"carrier-tenant\0verified\0" + tenant.encode()).hexdigest()
    return f"carrier-tenant:{digest}"


def _standards_drift(client: Any, **params: Any) -> dict[str, Any]:
    result = client._send("StandardsDrift", params, None)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _put_asset(
    client: Any,
    tenant: str,
    asset_id: str,
    facts: list[str],
    classes: tuple[str, ...] = ("database",),
) -> None:
    scope = carrier_scope(tenant)
    client.nodes.add(
        f"standards-asset:{scope}:{asset_id}",
        {
            "type": "StandardsAsset",
            "tenant_id": scope,
            "asset": {
                "id": asset_id,
                "tenant": scope,
                "capability_classes": list(classes),
                "facts": facts,
                "cost": 10,
            },
        },
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R701.3")
def test_cross_application_recommendation_is_a_proposal(clean_graph) -> None:
    scope = carrier_scope(TEST_TENANT)
    clean_graph.nodes.add(
        f"standard:{scope}:db-baseline",
        {
            "type": "Standard",
            "tenant_id": scope,
            "standard": {
                "id": "db-baseline",
                "capability_class": "database",
                "rules": [{"id": "tls", "required_fact": "tls", "weight": 1.0}],
            },
        },
    )
    _put_asset(clean_graph, TEST_TENANT, "orders-db", ["tls", "backup"])
    _put_asset(clean_graph, TEST_TENANT, "billing-db", ["tls", "backup"])
    _put_asset(clean_graph, OTHER_TENANT, "foreign-db", ["tls", "backup"])

    before = clean_graph.admin.audit_verify()["entries"]
    answer = _standards_drift(clean_graph)

    # One proposal naming only this tenant's two assets, and no score for the other tenant's.
    assert len(answer["recommendations"]) == 1
    recommendation = answer["recommendations"][0]
    assert recommendation["proposal_only"] is True
    assert recommendation["assets"] == ["billing-db", "orders-db"]
    assert recommendation["tenant"] == scope
    assert sorted(score["asset"] for score in answer["scores"]) == [
        "billing-db",
        "orders-db",
    ]
    assert all(score["standard"] == "db-baseline" for score in answer["scores"])
    assert [cohort["assets"] for cohort in answer["cohorts"]] == [
        ["billing-db", "orders-db"]
    ]

    # An unknown standard id refuses.
    with pytest.raises(EngineResponseError) as unknown:
        _standards_drift(clean_graph, standard="no-such-standard")
    assert "STANDARD_NOT_FOUND" in str(unknown.value)

    # Nothing was written: the graph audit chain did not move.
    assert clean_graph.admin.audit_verify()["entries"] == before


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R701.6")
def test_missing_standard_is_not_zero(clean_graph) -> None:
    scope = carrier_scope(TEST_TENANT)
    clean_graph.nodes.add(
        f"standard:{scope}:db-baseline",
        {
            "type": "Standard",
            "tenant_id": scope,
            "standard": {
                "id": "db-baseline",
                "capability_class": "database",
                "rules": [
                    {"id": "tls", "required_fact": "tls", "weight": 1.0},
                    {"id": "backup", "required_fact": "backup", "weight": 3.0},
                ],
            },
        },
    )
    # Two assets share the `database` class; the cache has no standard.
    _put_asset(clean_graph, TEST_TENANT, "orders-db", ["tls", "audit", "metrics"])
    _put_asset(
        clean_graph, TEST_TENANT, "billing-db", ["tls", "audit", "metrics", "backup"]
    )
    _put_asset(clean_graph, TEST_TENANT, "edge-cache", ["tls"], classes=("cache",))

    answer = _standards_drift(clean_graph)
    scores = {row["asset"]: row for row in answer["scores"]}

    # No standard covers the cache: unscored, with no numeric score and no standard.
    assert scores["edge-cache"]["score"] == {"state": "not_applicable"}
    assert scores["edge-cache"]["standard"] is None

    # Σ(weight × satisfied) / Σ weight: tls (1) of tls (1) + backup (3) is 0.25.
    partial = scores["orders-db"]["score"]
    assert partial["state"] == "scored"
    assert partial["score"] == pytest.approx(0.25)
    assert partial["satisfied"] == ["tls"]
    assert partial["failed"] == ["backup"]
    full = scores["billing-db"]["score"]
    assert full["score"] == pytest.approx(1.0)
    assert full["failed"] == []

    # The two assets sharing a capability class return one recommendation.
    assert len(answer["recommendations"]) == 1
    assert answer["recommendations"][0]["assets"] == ["billing-db", "orders-db"]
