"""Served source selection for suppliers, purchase items and third-party risk
(MEDULLA-CQ-XSRC-R002.5).

The seed `xsrc_authority_supplier.ttl` is attached under the operator origin, so its rows
are effective. The served `SourceCoverage` then selects the rank-1 source of each class.
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
SEED = (_ROOT / "crates/medulla-rdf/tests/fixtures/xsrc_authority_supplier.ttl").read_text()
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
