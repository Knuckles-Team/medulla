"""MEDULLA-FEDERATED-QUERY-R707.2 / R707.3 served acceptance: SPARQL 1.1
Update and the Graph Store verbs over the one served commit path.

Consumer of record (R707.3): arbora's ``/sparql`` route (ARBORA-OPS-R857.1)
-> the ``medulla.query`` client -> served ``Method::ApplyMutation`` with
event type ``SPARQL_HTTP_UPDATE_EVENT`` -> ``dispatch_sparql_http_update`` ->
``coordinated_sparql_http_update`` (one atomic ``MutationBatch``). The Graph
Store HTTP form maps its verbs to exactly these update queries for one named
graph — PUT = ``CLEAR DEFAULT; INSERT DATA`` (replace), POST = ``INSERT DATA``
(merge), DELETE = ``CLEAR DEFAULT`` (drop), GET = read — and re-enters
dispatch, so it shares this one commit path; there is no second update path.
"""

from __future__ import annotations

import pytest

SPARQL_UPDATE_EVENT = "sparql_http_update_v1"


def _update(client, query: str) -> dict:
    """Apply one SPARQL 1.1 Update through the served coordinated path."""
    return client.apply_mutation(SPARQL_UPDATE_EVENT, query)


def _canon(value: str) -> str:
    """Canonicalize one row value to its bracketed IRI form: the engine returns
    resource bindings bracketed and predicate (property-key) bindings bare."""
    return value if value.startswith("<") and value.endswith(">") else f"<{value}>"


def _triples(client) -> set[tuple[str, str, str]]:
    """The connection graph's triples as a set of canonical (s, p, o) row values."""
    rows = client.rdf.sparql("SELECT ?s ?p ?o WHERE { ?s ?p ?o }")
    return {(_canon(row["s"]), _canon(row["p"]), _canon(row["o"])) for row in rows}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R707.3")
def test_insert_then_select(clean_graph):
    """INSERT DATA via the served update event, then read it back with a served
    query."""
    report = _update(
        clean_graph, "INSERT DATA { <http://ex/a> <http://ex/p> <http://ex/b> }"
    )
    assert report["operations"] == 1
    assert report["inserted"] == 1

    rows = clean_graph.rdf.sparql("SELECT ?o WHERE { <http://ex/a> <http://ex/p> ?o }")
    assert [row["o"] for row in rows] == ["<http://ex/b>"]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R707.2")
def test_graph_store_verbs(clean_graph):
    """The Graph Store verb mapping (read / replace / merge / drop) over one graph."""
    first = "<http://ex/a> <http://ex/p> <http://ex/b> ."
    second = "<http://ex/c> <http://ex/p> <http://ex/d> ."

    # Seed the graph, then PUT — replace: the prior content is dropped.
    _update(clean_graph, f"INSERT DATA {{ {first} }}")
    assert _triples(clean_graph) == {
        ("<http://ex/a>", "<http://ex/p>", "<http://ex/b>")
    }
    _update(clean_graph, f"CLEAR DEFAULT; INSERT DATA {{ {second} }}")
    assert _triples(clean_graph) == {
        ("<http://ex/c>", "<http://ex/p>", "<http://ex/d>")
    }

    # POST — merge: the body joins the existing content.
    _update(clean_graph, f"INSERT DATA {{ {first} }}")
    assert _triples(clean_graph) == {
        ("<http://ex/a>", "<http://ex/p>", "<http://ex/b>"),
        ("<http://ex/c>", "<http://ex/p>", "<http://ex/d>"),
    }

    # DELETE — drop: the graph's content is gone, and a served read agrees.
    _update(clean_graph, "CLEAR DEFAULT")
    assert _triples(clean_graph) == set()
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == []
