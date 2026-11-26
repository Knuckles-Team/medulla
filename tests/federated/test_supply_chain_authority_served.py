"""Served source selection for suppliers, purchase items and third-party risk
(MEDULLA-CQ-XSRC-R002.5).

The seed `xsrc_authority_supplier.ttl` is attached under the operator origin, so its
rows are effective. The served `SourceCoverage` then selects the rank-1 source of
each class.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from test_source_coverage_served import (  # noqa: F401  (_detach_after is a fixture)
    _agent,
    _attach_document,
    _client,
    _coverage,
    _detach_after,
    _register,
)

_ROOT = Path(__file__).resolve().parents[2]
SEED = (
    _ROOT / "crates/medulla-rdf/tests/fixtures/xsrc_authority_supplier.ttl"
).read_text()
CORE = "https://knuckles-team.github.io/ontology/core#"
SUPPLIER = f"{CORE}Supplier"
ITEM = f"{CORE}SupplyChainItem"
RISK = f"{CORE}RiskScenario"
PRIVACY = f"{CORE}PrivacyAssessment"


def _verdict(reply: dict[str, Any], iri: str) -> dict[str, Any]:
    return next(v for v in reply["verdicts"] if v["class"] == iri)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R002.5")
def test_supplier_selects() -> None:
    owner = _client(_agent("supplier"))
    _register(owner, "erpnext-mcp", "leanix-mcp", "ciso-assistant-mcp", "onetrust-mcp")
    _attach_document(SEED)

    reply = _coverage(owner, [SUPPLIER, ITEM, RISK, PRIVACY])
    for iri in (SUPPLIER, ITEM):
        verdict = _verdict(reply, iri)
        assert verdict["verdict"] == "covered", verdict.get("premises")
        assert verdict["source"] == "erpnext-mcp"
        assert verdict["alternates"] == ["leanix-mcp"]
        assert verdict["premises"], "every verdict carries its premises"
    risk = _verdict(reply, RISK)
    assert (risk["verdict"], risk["source"]) == ("covered", "ciso-assistant-mcp")
    privacy = _verdict(reply, PRIVACY)
    assert (privacy["verdict"], privacy["source"]) == ("covered", "onetrust-mcp")
    used = {entry["source"] for entry in reply["audit"]["used"]}
    assert used == {"erpnext-mcp", "ciso-assistant-mcp", "onetrust-mcp"}

    # A row that names a source absent from the connected view abstains `unconnected`.
    other = _client(_agent("supplier-other"))
    absent = _verdict(_coverage(other, [SUPPLIER]), SUPPLIER)
    assert absent["verdict"] == "abstained", absent.get("premises")
    assert absent["reason"] == "unconnected", absent.get("premises")


INFRA = "https://knuckles-team.github.io/ontology/infrastructure#"
IMAGE = f"{INFRA}ContainerImage"
VULNERABILITY = f"{CORE}Vulnerability"
SERVICE = f"{INFRA}Service"
HOST = f"{INFRA}Host"
CAPABILITY = f"{CORE}BusinessCapability"
HEALTH = f"{INFRA}serviceStatus"
LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
TEMPERATURE = f"{INFRA}temperatureCelsius"


def _seed(name: str) -> str:
    return (
        _ROOT / f"crates/medulla-rdf/tests/fixtures/xsrc_authority_{name}.ttl"
    ).read_text()


def _facet_verdict(reply: dict[str, Any], iri: str, facet: str) -> dict[str, Any]:
    return next(
        v for v in reply["verdicts"] if v["class"] == iri and v.get("facet") == facet
    )


def _facets(*pairs: tuple[str, str]) -> list[dict[str, str]]:
    return [{"class": cls, "property": prop} for cls, prop in pairs]


def _assert_unconnected(iri: str) -> None:
    """A row naming a source absent from the connected view abstains `unconnected`."""
    other = _client(_agent("seed-other"))
    absent = _verdict(_coverage(other, [iri]), iri)
    assert absent["verdict"] == "abstained", absent.get("premises")
    assert absent["reason"] == "unconnected", absent.get("premises")


@pytest.mark.spec("MEDULLA-CQ-XSRC-R002.6")
def test_component_selects() -> None:
    owner = _client(_agent("component"))
    _register(owner, "dockerhub-mcp", "gitlab-mcp", "github-mcp")
    _attach_document(_seed("component"))

    reply = _coverage(owner, [IMAGE, VULNERABILITY])
    image = _verdict(reply, IMAGE)
    assert (image["verdict"], image["source"]) == ("covered", "dockerhub-mcp")
    assert image["premises"], "every verdict carries its premises"
    vulnerability = _verdict(reply, VULNERABILITY)
    assert (vulnerability["verdict"], vulnerability["source"]) == (
        "covered",
        "gitlab-mcp",
    )
    assert vulnerability["alternates"] == ["github-mcp"]
    used = {entry["source"] for entry in reply["audit"]["used"]}
    assert used == {"dockerhub-mcp", "gitlab-mcp"}
    _assert_unconnected(IMAGE)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.1")
def test_service_selects() -> None:
    owner = _client(_agent("service"))
    _register(owner, "uptime-mcp", "lgtm-mcp", "servicenow-mcp")
    _attach_document(_seed("service"))

    reply = _coverage(
        owner, [SERVICE], facets=_facets((SERVICE, HEALTH), (SERVICE, LABEL))
    )
    health = _facet_verdict(reply, SERVICE, HEALTH)
    assert (health["verdict"], health["source"]) == ("covered", "uptime-mcp")
    assert health["alternates"] == ["lgtm-mcp"]
    identity = _facet_verdict(reply, SERVICE, LABEL)
    assert (identity["verdict"], identity["source"]) == ("covered", "servicenow-mcp")
    assert health["premises"] and identity["premises"]
    _assert_unconnected(SERVICE)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.2")
def test_workload_selects() -> None:
    owner = _client(_agent("workload"))
    _register(owner, "container-manager-mcp", "portainer-mcp", "systems-manager-mcp")
    _attach_document(_seed("workload"))

    reply = _coverage(owner, [IMAGE, HOST], facets=_facets((HOST, TEMPERATURE)))
    for iri in (IMAGE, HOST):
        verdict = _verdict(reply, iri)
        assert verdict["verdict"] == "covered", verdict.get("premises")
        assert verdict["source"] == "container-manager-mcp"
        assert verdict["alternates"] == ["portainer-mcp"]
    hardware = _facet_verdict(reply, HOST, TEMPERATURE)
    assert (hardware["verdict"], hardware["source"]) == (
        "covered",
        "systems-manager-mcp",
    )
    _assert_unconnected(IMAGE)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.3")
def test_capability_selects() -> None:
    owner = _client(_agent("capability"))
    _register(owner, "leanix-mcp", "archimate-mcp")
    _attach_document(_seed("capability"))

    verdict = _verdict(_coverage(owner, [CAPABILITY]), CAPABILITY)
    assert verdict["verdict"] == "covered", verdict.get("premises")
    assert verdict["source"] == "leanix-mcp"
    assert verdict["alternates"] == ["archimate-mcp"]
    assert verdict["premises"], "every verdict carries its premises"
    _assert_unconnected(CAPABILITY)
