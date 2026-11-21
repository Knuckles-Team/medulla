"""Workload-aware column vectorization advice, served (MEDULLA-FEDERATED-QUERY-R077.6).

Real text-search statements run over the wire; ``information_schema.vector_advice``
then ranks the columns by the traffic the tenant's reads recorded.
"""

from __future__ import annotations

import uuid

import pytest

from medulla.client import EngineResponseError


def _table(clean_graph) -> str:
    name = f"advice_{uuid.uuid4().hex[:10]}"
    clean_graph.query.sql(
        f"CREATE TABLE {name} (id BIGINT PRIMARY KEY, title TEXT, body TEXT, qty "
        f"BIGINT)"
    )
    clean_graph.query.sql(
        f"INSERT INTO {name} VALUES (1, 'alpha', 'first long body text', 1), "
        f"(2, 'beta', 'second long body text', 2)"
    )
    return name


def _advice(clean_graph, table: str) -> dict[str, dict]:
    rows = clean_graph.query.sql(
        "SELECT table_name, column_name, eligibility, reason, score, sample_count "
        "FROM information_schema.vector_advice"
    )
    return {r["column_name"]: r for r in rows if r["table_name"] == table}


def test_advice_relation_reads_production_observations(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.6.3"""
    table = _table(clean_graph)
    before = _advice(clean_graph, table)
    assert before["body"]["sample_count"] == 0
    for _ in range(3):
        clean_graph.query.sql(f"SELECT id FROM {table} WHERE body LIKE '%long%'")
    clean_graph.query.sql(f"SELECT id FROM {table} WHERE qty = 1")
    after = _advice(clean_graph, table)
    assert int(after["body"]["sample_count"]) == 3
    assert int(after["title"]["sample_count"]) == 0
    assert int(after["qty"]["sample_count"]) == 0


def test_unembeddable_types_refuse_without_side_effects(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.6.4"""
    table = _table(clean_graph)
    listing = _advice(clean_graph, table)
    assert listing["qty"]["eligibility"] == "unsupported_type"
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.sql(
            "SELECT * FROM information_schema.vector_advice "
            f"WHERE table_name = '{table}' AND column_name = 'qty'"
        )
    assert "COLUMN_TYPE_UNSUPPORTED" in str(refused.value)
    # the refusal left the listing intact and created no binding
    assert _advice(clean_graph, table)["body"]["eligibility"] in {
        "eligible",
        "insufficient_observations",
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.2")
def test_workload_changes_advice(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.6.2: the ranking follows the traffic, not the statistics."""
    table = _table(clean_graph)
    idle = _advice(clean_graph, table)
    assert float(idle["body"]["score"]) == 0.0
    assert float(idle["title"]["score"]) == 0.0

    for _ in range(60):
        clean_graph.query.sql(f"SELECT id FROM {table} WHERE body LIKE '%long%'")
    body_first = _advice(clean_graph, table)
    assert body_first["body"]["eligibility"] == "eligible"
    assert float(body_first["body"]["score"]) > float(body_first["title"]["score"])
    assert float(body_first["title"]["score"]) == 0.0
    # the integer column stays out of the ranking whatever the traffic
    assert body_first["qty"]["eligibility"] == "unsupported_type"

    for _ in range(180):
        clean_graph.query.sql(f"SELECT id FROM {table} WHERE title LIKE '%a%'")
    title_first = _advice(clean_graph, table)
    assert float(title_first["title"]["score"]) > float(title_first["body"]["score"])
    assert float(title_first["body"]["score"]) > 0.0
