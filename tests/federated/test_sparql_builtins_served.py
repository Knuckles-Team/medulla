"""MEDULLA-FEDERATED-QUERY-R742 served acceptance: one query builds identifiers with the
SPARQL 1.1 string builtins.

The query composes `REPLACE`, `ENCODE_FOR_URI`, `CONCAT` and `IRI` into one IRI, and
resolves a relative reference with `IRI` under a query `BASE` (RFC 3986). A relative
reference with no `BASE` is an error value, so its variable stays unbound.
"""

from __future__ import annotations

import pytest
from _sparql_solutions import solution_values


def _bare(value: str | None) -> str | None:
    """A resource binding without its angle brackets."""
    if value is None:
        return None
    return value[1:-1] if value.startswith("<") and value.endswith(">") else value


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R742")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R742.2")
def test_identifier_building_query(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R742.2: the answer holds the IRI built from
    `CONCAT`, `ENCODE_FOR_URI` and `REPLACE`, and the reference `IRI` resolves under a
    `BASE`."""
    rows = solution_values(
        clean_graph.rdf.sparql(
            "BASE <http://ex/dir/> "
            "SELECT ?built ?resolved ?parent WHERE { "
            'BIND(REPLACE("  Los Angeles/CA", "^ +", "") AS ?name) '
            'BIND(IRI(CONCAT("http://ex/city/", ENCODE_FOR_URI(?name))) AS ?built) '
            'BIND(IRI("child") AS ?resolved) '
            'BIND(IRI("../up") AS ?parent) }'
        )
    )
    assert len(rows) == 1, rows
    row = rows[0]
    assert _bare(row["built"]) == "http://ex/city/Los%20Angeles%2FCA"
    assert _bare(row["resolved"]) == "http://ex/dir/child"
    assert _bare(row["parent"]) == "http://ex/up"


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R742.2")
def test_relative_reference_without_a_base_is_unbound(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R742.2: `IRI` of a relative reference with no
    `BASE` is an error value, so the variable is unbound and the absolute one is not."""
    rows = solution_values(
        clean_graph.rdf.sparql(
            'SELECT ?relative ?absolute WHERE { BIND(IRI("child") AS ?relative) '
            'BIND(IRI("http://ex/child") AS ?absolute) }'
        )
    )
    assert len(rows) == 1, rows
    assert rows[0].get("relative") is None
    assert _bare(rows[0]["absolute"]) == "http://ex/child"
