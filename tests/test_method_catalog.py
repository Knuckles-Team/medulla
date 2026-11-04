"""The installed engine serves its method catalog (MEDULLA-CONTRACT-R057)."""

from __future__ import annotations

import json

import pytest


@pytest.mark.spec("MEDULLA-CONTRACT-R057")
def test_installed_engine_serves_the_method_catalog():
    engine = pytest.importorskip("medulla.engine")
    catalog = json.loads(engine.method_catalog())
    assert isinstance(catalog["catalog_digest"], str) and catalog["catalog_digest"]
    scopes = {row["scope"]: row["class"] for row in catalog["scopes"]}
    assert scopes
    methods = catalog["methods"]
    assert methods
    ids = [row["id"] for row in methods]
    assert len(ids) == len(set(ids))
    for row in methods:
        assert row["policy"]["authz_action"] in scopes, row["id"]
        assert isinstance(row["policy"]["mutates"], bool)
        assert isinstance(row["policy"]["idempotent"], bool)
        assert row["replay_class"] in {
            "OperationIdentity",
            "NonceOnly",
            "NotReplayable",
        }
        assert row["stability"] in {"stable", "internal", "deprecated"}
    assert any(row["is_wire_callable"] for row in methods)
