"""MEDULLA-UNIFIED-DATA-PLANE-R038.2 served acceptance: discovery writes a contract.

Registering a source writes one ``MetadataContract`` node (the source kind, name,
schema version and the SHA-256 of its canonical schema) and no entity rows. A source
whose catalog cannot be read refuses with ``DISCOVERY_UNREACHABLE`` and leaves no
catalog entry.
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError


def _contracts(clean_graph) -> list[tuple[str, dict]]:
    return [
        (node_id, props)
        for node_id, props in clean_graph.nodes.list_by_label("MetadataContract", 0)
    ]


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.2.1")
def test_registration_writes_contract_only(clean_graph, pg_source):
    pg_source.execute("DROP TABLE IF EXISTS accel_orders")
    pg_source.execute("CREATE TABLE accel_orders (id int primary key, total numeric)")
    pg_source.execute("INSERT INTO accel_orders VALUES (1, 10), (2, 20), (3, 30)")
    before = clean_graph.nodes.count()
    registered = clean_graph.query.register_foreign_source(
        "accel-orders",
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": "SELECT id, total FROM accel_orders",
                "id_field": "id",
                "columns": ["total"],
            }
        },
    )
    assert registered == "accel-orders"
    contracts = _contracts(clean_graph)
    assert len(contracts) == 1, "one contract node per registered source"
    _, props = contracts[0]
    assert props["source_kind"] == "postgres_wire"
    assert props["source_name"] == "accel-orders"
    assert props["schema_version"] == 1
    digest = props["content_hash"]
    assert len(digest) == 64 and all(c in "0123456789abcdef" for c in digest)
    # Zero entity rows: the contract is the only node the registration added.
    assert clean_graph.nodes.count() == before + 1
    # Re-registering an unchanged source rewrites nothing and keeps the version.
    clean_graph.query.register_foreign_source(
        "accel-orders",
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": "SELECT id, total FROM accel_orders",
                "id_field": "id",
                "columns": ["total"],
            }
        },
    )
    again = _contracts(clean_graph)
    assert len(again) == 1 and again[0][1]["schema_version"] == 1
    # A changed schema raises the version by one.
    pg_source.execute("ALTER TABLE accel_orders ADD COLUMN note text")
    clean_graph.query.register_foreign_source(
        "accel-orders",
        {
            "Sql": {
                "dsn": pg_source.dsn,
                "query": "SELECT id, total FROM accel_orders",
                "id_field": "id",
                "columns": ["total"],
            }
        },
    )
    changed = _contracts(clean_graph)
    assert changed[0][1]["schema_version"] == 2
    assert changed[0][1]["content_hash"] != digest


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.2.1")
def test_unreachable_source_registers_nothing(clean_graph, dead_pg_dsn):
    dead = dead_pg_dsn
    before = clean_graph.nodes.count()
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.register_foreign_source(
            "accel-dead",
            {
                "Sql": {
                    "dsn": dead,
                    "query": "SELECT id FROM t",
                    "id_field": "id",
                }
            },
        )
    assert "DISCOVERY_UNREACHABLE" in str(refused.value)
    assert "accel-dead" in str(refused.value)
    assert clean_graph.nodes.count() == before
    assert _contracts(clean_graph) == []
    # No catalog entry: a unified read of the name is refused as unknown.
    with pytest.raises(EngineResponseError):
        clean_graph.query.unified(
            [
                {
                    "ForeignScan": {
                        "source": {"Named": {"name": "accel-dead"}},
                        "join": False,
                    }
                }
            ]
        )
