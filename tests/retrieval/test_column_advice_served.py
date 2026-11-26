"""Workload-aware column vectorization advice, served (MEDULLA-FEDERATED-QUERY-R077.6).

Real text-search statements run over the wire; ``information_schema.vector_advice``
then ranks the columns by the traffic the tenant's reads recorded.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import tenant_request_context

from medulla.client import EngineResponseError, SyncMedullaClient

OTHER_TENANT = "tenant:advice-other"


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


# proves R077.6.3: Column advice: usage recording and the advice relation
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.3")
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


# proves R077.6.4: Column advice: refusal scoped to the question
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.4")
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


# proves R077.6.6: Column advice: hot-path proof
# proves R077.6: Workload-aware column vectorization advice
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.6")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.2")
def test_workload_changes_advice(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.6.2: the ranking follows the traffic, not the
    statistics."""
    table = _table(clean_graph)
    indexes_before = _index_names(clean_graph)
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

    # unembeddable refusal: a pinned read of the integer column refuses, and the
    # listing of the other columns is unaffected
    with pytest.raises(EngineResponseError) as unsupported:
        clean_graph.query.sql(
            "SELECT * FROM information_schema.vector_advice "
            f"WHERE table_name = '{table}' AND column_name = 'qty'"
        )
    assert "COLUMN_TYPE_UNSUPPORTED" in str(unsupported.value)
    assert _advice(clean_graph, table)["body"]["eligibility"] == "eligible"

    # tenant isolation: another tenant's table of the same name starts from no
    # observations, its traffic never moves this tenant's counters, and this
    # tenant's traffic never reaches it
    ours = int(_advice(clean_graph, table)["body"]["sample_count"])
    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=tenant_request_context(OTHER_TENANT),
    )
    try:
        other.query.sql(
            f"CREATE TABLE {table} (id BIGINT PRIMARY KEY, title TEXT, body TEXT, qty "
            f"BIGINT)"
        )
        other.query.sql(
            f"INSERT INTO {table} VALUES (1, 'alpha', 'first long body', 1)"
        )
        assert int(_advice(other, table)["body"]["sample_count"]) == 0
        for _ in range(5):
            other.query.sql(f"SELECT id FROM {table} WHERE body LIKE '%long%'")
        assert int(_advice(other, table)["body"]["sample_count"]) == 5
        assert int(_advice(clean_graph, table)["body"]["sample_count"]) == ours
    finally:
        other.close()

    # zero automatic bindings: no amount of traffic created or queued a binding
    assert _index_names(clean_graph) == indexes_before

    # stale schema: a DDL change moves the catalog revision, so the counters seen
    # under the old revision rank as stale. A pinned read refuses; a listing does
    # not refuse on account of that column and still shows every column
    clean_graph.query.sql(f"CREATE TABLE {table}_moved (id BIGINT PRIMARY KEY)")
    stale = _advice(clean_graph, table)
    assert stale["body"]["eligibility"] == "insufficient_observations"
    assert stale["body"]["reason"] == "SCHEMA_REVISION_STALE"
    assert float(stale["body"]["score"]) == 0.0
    assert set(stale) == {"id", "title", "body", "qty"}
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.sql(
            "SELECT * FROM information_schema.vector_advice "
            f"WHERE table_name = '{table}' AND column_name = 'body'"
        )
    assert "SCHEMA_REVISION_STALE" in str(refused.value)
    assert _index_names(clean_graph) == indexes_before


def _receipt_rows(clean_graph, table: str) -> dict[str, dict]:
    rows = clean_graph.query.sql(
        "SELECT table_name, column_name, eligibility, score, sample_count, "
        "total_text_search_uses, policy_revision, demand_contribution, "
        "length_contribution, distinct_contribution "
        "FROM information_schema.vector_advice"
    )
    return {r["column_name"]: r for r in rows if r["table_name"] == table}


def _index_names(clean_graph) -> list[str]:
    rows = clean_graph.query.sql(
        "SELECT index_name FROM information_schema.medulla_index_status"
    )
    return sorted(r["index_name"] for r in rows)


# proves R077.6.5: Column advice: evidence
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.6.5")
def test_receipt_matches_observations_binding_unchanged(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.6.5: the receipt explains the score from the uses
    issued."""
    table = _table(clean_graph)
    indexes_before = _index_names(clean_graph)
    for _ in range(40):
        clean_graph.query.sql(f"SELECT id FROM {table} WHERE body LIKE '%long%'")
    for _ in range(20):
        clean_graph.query.sql(f"SELECT id FROM {table} WHERE title LIKE '%a%'")
    rows = _receipt_rows(clean_graph, table)
    body, title = rows["body"], rows["title"]
    assert body["eligibility"] == "eligible"
    assert int(body["sample_count"]) == 40
    assert int(title["sample_count"]) == 20
    for row in (body, title):
        assert int(row["total_text_search_uses"]) == int(
            rows["body"]["sample_count"]
        ) + int(rows["title"]["sample_count"])
        assert row["policy_revision"].startswith("sql_advice/v1 min_samples=")
        parts = (
            float(row["demand_contribution"])
            + float(row["length_contribution"])
            + float(row["distinct_contribution"])
        )
        assert abs(parts - float(row["score"])) < 1e-9
    assert (
        float(body["demand_contribution"]) > float(title["demand_contribution"]) > 0.0
    )
    # a column that is not eligible carries zero contributions
    qty = rows["qty"]
    assert qty["eligibility"] == "unsupported_type"
    assert float(qty["demand_contribution"]) == 0.0
    assert float(qty["length_contribution"]) == 0.0
    assert float(qty["distinct_contribution"]) == 0.0
    assert _index_names(clean_graph) == indexes_before
