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
    """The test's triples (subjects ex:a and ex:c) as canonical (s, p, o) values.

    Served SPARQL also merges every visible pack graph of the tenant into the
    default graph (MEDULLA-MEDIA-VOCAB-R655), so the read names the subjects
    this test writes instead of listing every triple.
    """
    rows = client.rdf.sparql(
        "SELECT ?s ?p ?o WHERE { VALUES ?s { <http://ex/a> <http://ex/c> } ?s ?p ?o }"
    )
    return {(_canon(row["s"]), _canon(row["p"]), _canon(row["o"])) for row in rows}


# spec: MEDULLA-FEDERATED-QUERY-R707.3
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
    assert clean_graph.nodes.ids() == []


# spec: MEDULLA-FEDERATED-QUERY-R707.4
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R707.4")
def test_update_refusals(clean_graph):
    """An update from a caller with no write access and a LOAD from a private address
    both refuse, and each leaves the store unchanged."""
    import asyncio
    import os
    import uuid

    from conftest import register_commons_agents, request_context

    from medulla.client import EngineResponseError, MedullaClient

    _update(clean_graph, "INSERT DATA { <http://ex/a> <http://ex/p> <http://ex/b> }")
    before = _triples(clean_graph)
    assert before

    # LOAD from a private address refuses through the shared outbound guard.
    for target in ("http://127.0.0.1/data.nt", "https://10.0.0.5/data.nt"):
        with pytest.raises(Exception) as refused:
            _update(clean_graph, f"LOAD <{target}> INTO GRAPH <http://ex/loaded>")
        assert "federation" in str(refused.value).lower() or "LOAD" in str(refused.value)
    assert _triples(clean_graph) == before

    # A caller that holds only read scope may not update.
    reader = f"sparql-reader-{uuid.uuid4().hex[:8]}"
    register_commons_agents(reader)

    async def attempt() -> str:
        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=reader, scopes=["kg:read"]),
        )
        try:
            await client.apply_mutation(
                SPARQL_UPDATE_EVENT,
                "INSERT DATA { <http://ex/c> <http://ex/p> <http://ex/d> }",
            )
        except EngineResponseError as error:
            return str(error)
        finally:
            await client.close()
        return ""

    refusal = asyncio.run(attempt())
    assert refusal, "an update without write access must refuse"
    assert _triples(clean_graph) == before


# spec: MEDULLA-FEDERATED-QUERY-R707.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R707.5")
def test_update_receipt_reads_back(clean_graph):
    """The report carries the SHA-256 of the request and the committed batch id."""
    import hashlib
    import re

    query = "INSERT DATA { <http://ex/a> <http://ex/p> <http://ex/b> }"
    report = _update(clean_graph, query)
    assert report["request_sha256"] == hashlib.sha256(query.encode()).hexdigest()
    assert re.fullmatch(r"[0-9a-f]{64}", report["request_sha256"])
    assert report["batch_id"]
    assert report["inserted"] == 1

    # The same text is the same digest; different text is a different one.
    again = _update(clean_graph, query)
    assert again["request_sha256"] == report["request_sha256"]
    other = _update(clean_graph, "INSERT DATA { <http://ex/c> <http://ex/p> <http://ex/d> }")
    assert other["request_sha256"] != report["request_sha256"]
