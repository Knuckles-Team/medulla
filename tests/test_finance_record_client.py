"""Finance record client round trips against the Rust-emitted vectors.

The vectors in ``tests/fixtures/finance_record_vectors.json`` are written by
``crates/medulla-types/src/wire_finance.rs::tests::emit_record_vectors`` from the
Rust serde types. Every vector is encoded by the client wrappers and sent
through the session-scoped served engine (conftest.py).
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from conftest import TEST_TENANT

VECTORS = json.loads(
    (Path(__file__).parent / "fixtures" / "finance_record_vectors.json").read_text()
)["vectors"]
RUST_TENANT = "integration-test-tenant"


def _retenant(record: dict[str, Any]) -> dict[str, Any]:
    """The vectors carry the Rust served-test tenant; swap in the conftest tenant."""
    fields = {
        name: value.replace(RUST_TENANT, TEST_TENANT)
        for name, value in record["fields"].items()
    }
    return {**record, "tenant": TEST_TENANT, "fields": fields}


def test_vectors_cover_all_eight_records() -> None:
    assert len(VECTORS) == 8
    assert len({vector["kind"] for vector in VECTORS}) == 8


# spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.3.2
@pytest.mark.parametrize("vector", VECTORS, ids=lambda vector: vector["kind"])
def test_record_round_trips_against_rust_vectors(clean_graph, vector) -> None:
    record = _retenant(vector["record"])
    kind = vector["kind"]
    if not vector["client_writable"]:
        with pytest.raises(Exception, match="RECORD_DERIVED_READONLY"):
            clean_graph.finance.record_put(kind, vector["schema_version"], record)
        return
    clean_graph.finance.record_put(kind, vector["schema_version"], record)
    stored = clean_graph.finance.record_get(kind, record["record_id"])
    assert stored["fields"] == record["fields"]
    assert stored["tenant"] == record["tenant"]
    assert stored["schema_version"] == vector["schema_version"]
    with pytest.raises(Exception, match="FINANCE_RECORD_VERSION_UNKNOWN"):
        clean_graph.finance.record_put(kind, "finance-v0", record)
    broken = {**record, "fields": {}}
    with pytest.raises(Exception, match="RECORD_SHAPE_INVALID"):
        clean_graph.finance.record_put(kind, vector["schema_version"], broken)
