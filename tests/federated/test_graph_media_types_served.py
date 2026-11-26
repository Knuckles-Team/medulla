"""MEDULLA-FEDERATED-QUERY-R740 served acceptance: one graph is written and read in each
of the six RDF media types.

A write is a served ``Method::ApplyMutation`` with event type ``rdf_graph_write``
whose ``query`` is one JSON object ``{media_type, mode, body}``; it parses with the
function the graph-store route uses and commits through the SPARQL update
coordinator. A read is a served ``Method::Sparql`` whose ``graph_media_type`` names
the type of the ``graph_body`` of a ``CONSTRUCT`` answer. The round trip never leaves
the engine: each type's body is written, read back in that type, written again and
read back as N-Triples.
"""

from __future__ import annotations

import json

import pytest

RDF_WRITE_EVENT = "rdf_graph_write"
SPARQL_UPDATE_EVENT = "sparql_http_update_v1"

CONSTRUCT = (
    "CONSTRUCT { ?s ?p ?o } "
    "WHERE { VALUES ?s { <http://ex/a> <http://ex/b> } ?s ?p ?o }"
)

# One graph, in each media type: two triples about ex:a and one about ex:b.
EXPECTED = {
    "<http://ex/a> <http://ex/knows> <http://ex/b> .",
    '<http://ex/a> <http://ex/name> "Alice" .',
    '<http://ex/b> <http://ex/name> "Bob" .',
}
BODIES = {
    "application/n-triples": "\n".join(sorted(EXPECTED)) + "\n",
    "text/turtle": (
        "@prefix ex: <http://ex/> .\n"
        'ex:a ex:knows ex:b ; ex:name "Alice" .\n'
        'ex:b ex:name "Bob" .\n'
    ),
    "application/n-quads": "\n".join(sorted(EXPECTED)) + "\n",
    "application/trig": (
        "@prefix ex: <http://ex/> .\n"
        '{ ex:a ex:knows ex:b ; ex:name "Alice" . ex:b ex:name "Bob" . }\n'
    ),
    "application/ld+json": json.dumps(
        [
            {
                "@id": "http://ex/a",
                "http://ex/knows": [{"@id": "http://ex/b"}],
                "http://ex/name": "Alice",
            },
            {"@id": "http://ex/b", "http://ex/name": "Bob"},
        ]
    ),
    "application/rdf+xml": (
        '<?xml version="1.0"?>'
        '<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" '
        'xmlns:ex="http://ex/">'
        '<rdf:Description rdf:about="http://ex/a">'
        '<ex:knows rdf:resource="http://ex/b"/><ex:name>Alice</ex:name>'
        "</rdf:Description>"
        '<rdf:Description rdf:about="http://ex/b"><ex:name>Bob</ex:name>'
        "</rdf:Description></rdf:RDF>"
    ),
}


def _write(client, media_type: str, body: str, mode: str = "replace") -> dict:
    envelope = json.dumps({"media_type": media_type, "mode": mode, "body": body})
    return client.apply_mutation(RDF_WRITE_EVENT, envelope)


def _read(client, media_type: str) -> str:
    result = client.rdf.sparql_result(CONSTRUCT, graph_media_type=media_type)
    assert result.graph_body is not None, media_type
    return result.graph_body


def _triples(client) -> set[str]:
    """The graph as a set of N-Triples lines."""
    return {
        line for line in _read(client, "application/n-triples").splitlines() if line
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R740")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R740.4")
def test_each_media_type_round_trips(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R740.4: each of the six types writes the graph,
    reads back in its own type, and that read writes the same graph again."""
    for media_type, body in BODIES.items():
        _write(clean_graph, media_type, body)
        assert _triples(clean_graph) == EXPECTED, media_type
        echoed = _read(clean_graph, media_type)
        assert echoed.strip(), media_type
        # Replace with the engine's own serialization: the same triple set comes back.
        _write(clean_graph, "application/n-triples", "")
        assert _triples(clean_graph) == set(), media_type
        _write(clean_graph, media_type, echoed)
        assert _triples(clean_graph) == EXPECTED, media_type


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R740.4")
def test_refusals_leave_the_store_unchanged(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R740.4: the refusals of R740.1 reach the served
    write with their codes, and a read of an unsupported type refuses before it runs."""
    _write(clean_graph, "application/n-triples", BODIES["application/n-triples"])
    before = _triples(clean_graph)
    assert before == EXPECTED

    other_graph = '<http://ex/a> <http://ex/name> "Eve" <http://ex/other> .\n'
    refused = {
        "RDF_MEDIA_TYPE_UNSUPPORTED": ("text/plain", "anything"),
        "RDF_PARSE_FAILED": ("text/turtle", "ex:a ex:p"),
        "RDF_DATASET_GRAPH_MISMATCH": ("application/n-quads", other_graph),
    }
    for code, (media_type, body) in refused.items():
        with pytest.raises(Exception, match=code):
            _write(clean_graph, media_type, body)
        assert _triples(clean_graph) == before, code

    with pytest.raises(Exception, match="RDF_PARSE_FAILED"):
        clean_graph.apply_mutation(RDF_WRITE_EVENT, "not json")
    with pytest.raises(Exception, match="RDF_MEDIA_TYPE_UNSUPPORTED"):
        clean_graph.rdf.sparql_result(CONSTRUCT, graph_media_type="text/plain")
    assert _triples(clean_graph) == before


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R740.3")
def test_write_event_and_http_route_share_one_parser(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R740.3: the served event gives the store the
    graph-store route's update gives for the same Turtle body (PUT = replace, POST =
    merge), because both build the update text with one function."""
    turtle = BODIES["text/turtle"]
    ntriples = "\n".join(sorted(EXPECTED))

    # PUT = replace: the route's update text is `CLEAR DEFAULT; INSERT DATA {...}`.
    clean_graph.apply_mutation(
        SPARQL_UPDATE_EVENT,
        'INSERT DATA { <http://ex/z> <http://ex/p> "z" }',
    )
    clean_graph.apply_mutation(
        SPARQL_UPDATE_EVENT, f"CLEAR DEFAULT; INSERT DATA {{\n{ntriples}\n}}"
    )
    by_route = _triples(clean_graph)
    clean_graph.apply_mutation(SPARQL_UPDATE_EVENT, "CLEAR DEFAULT")
    assert _triples(clean_graph) == set()

    clean_graph.apply_mutation(
        SPARQL_UPDATE_EVENT, 'INSERT DATA { <http://ex/z> <http://ex/p> "z" }'
    )
    _write(clean_graph, "text/turtle", turtle, mode="replace")
    assert _triples(clean_graph) == by_route == EXPECTED

    # POST = merge keeps what the graph held and adds the body.
    clean_graph.apply_mutation(SPARQL_UPDATE_EVENT, "CLEAR DEFAULT")
    _write(
        clean_graph, "application/n-triples", '<http://ex/a> <http://ex/name> "Alice" .'
    )
    _write(clean_graph, "text/turtle", turtle, mode="merge")
    assert _triples(clean_graph) == EXPECTED
