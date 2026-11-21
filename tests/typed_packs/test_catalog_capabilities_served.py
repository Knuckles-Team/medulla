"""Served catalog search (MEDULLA-TYPED-PACKS-R108.16).

One ranked relevance function covers every asset family. It is reached through the
served ``Sparql`` method as ``cat:relevance(?asset, "text")``; the usage prior is
capped (the counts come from the usage series) and the trust flags scale the score.
"""

from __future__ import annotations

import hashlib
import os
import time
import uuid

import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
PREFIX = (
    f"@prefix cat: <{CATALOG}> .\n"
    "@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n"
)
SEARCH = (
    f"PREFIX cat: <{CATALOG}>\n"
    "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n"
    "SELECT ?asset ?score WHERE {{\n"
    "  ?asset skos:prefLabel ?name .\n"
    '  BIND(cat:relevance(?asset, "{text}") AS ?score)\n'
    "  FILTER(?score > 0)\n"
    "}} ORDER BY DESC(?score)"
)
FAMILIES = {
    "tool": "ToolAsset",
    "prompt": "PromptAsset",
    "skill": "SkillAsset",
    "resource": "DataResource",
}


@pytest.fixture
def catalog_graph():
    graph = f"medulla-r10816-{uuid.uuid4().hex}"
    admin = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(scopes=None),
    )
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        yield admin
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


def _ranked(admin, text: str) -> list[tuple[str, float]]:
    rows = admin.rdf.sparql(SEARCH.format(text=text))
    return [(row["asset"], float(row["score"])) for row in rows]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.16")
def test_one_search_ranks_all_families_with_usage_and_endorsement_boost(catalog_graph):
    # spec: MEDULLA-TYPED-PACKS-R108.16
    admin = catalog_graph
    lines = [PREFIX]
    for family, cls in FAMILIES.items():
        lines.append(
            f'<urn:medulla:asset:{family}:orders> a cat:{cls} ; skos:prefLabel "orders {family}" .\n'
        )
    admin.rdf.add_triples(turtle="".join(lines))

    # Every family is found through the one function.
    found = {asset for asset, _ in _ranked(admin, "orders")}
    assert found == {
        f"<urn:medulla:asset:{family}:orders>" for family in FAMILIES
    } or found == {f"urn:medulla:asset:{family}:orders" for family in FAMILIES}

    # An asset that matches nothing is not a hit.
    assert _ranked(admin, "zzz-no-such-asset") == []

    # A deprecation (cat:flag) lowers the rank of the asset it is set on.
    baseline = dict(_ranked(admin, "orders"))
    skill = "urn:medulla:asset:skill:orders"
    admin.rdf.add_triples(turtle=f'{PREFIX}<{skill}> cat:flag "deprecated" .\n')
    after_deprecate = dict(_ranked(admin, "orders"))
    key = next(k for k in after_deprecate if k.strip("<>") == skill)
    base_key = next(k for k in baseline if k.strip("<>") == skill)
    assert after_deprecate[key] < baseline[base_key]

    # Text over the knob is refused with its code.
    with pytest.raises(Exception, match="CATALOG_QUERY_TOO_LONG"):
        _ranked(admin, "x" * 1001)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.14")
def test_dashboard_report_saved_query_and_template_register_with_reads_and_usage(
    catalog_graph,
):
    # spec: MEDULLA-TYPED-PACKS-R108.14
    admin = catalog_graph
    xsd_int = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
    owner = "<urn:principal:data-team>"
    admin.rdf.add_triples(
        turtle=(
            f"{PREFIX}"
            f'<urn:medulla:asset:resource:sales> a cat:DataResource ; skos:prefLabel "sales resource" ; '
            f'cat:flag "owner_missing" ; <https://knuckles-team.github.io/ontology/connector-pack#ofConnector> <urn:c:a> ; '
            f"<https://knuckles-team.github.io/ontology/connector-pack#servesClass> <urn:class:Sale> .\n"
            f'<urn:medulla:asset:dashboard:sales> a cat:DashboardAsset ; skos:prefLabel "sales dashboard" ; '
            f'cat:hasOwner {owner} ; cat:revision "1"^^<{xsd_int}> ; cat:readsAsset <urn:medulla:asset:resource:sales> .\n'
            f'<urn:medulla:asset:report:sales> a cat:ReportAsset ; skos:prefLabel "sales report" ; '
            f"cat:hasOwner {owner} ; cat:readsAsset <urn:medulla:asset:resource:sales> .\n"
            f'<urn:medulla:asset:template:sales> a cat:ReportTemplate ; skos:prefLabel "sales template" ; '
            f'cat:hasOwner {owner} ; cat:parameterName "region" ; cat:readsAsset <urn:medulla:asset:resource:sales> .\n'
        )
    )
    # Each reporting asset lists what it reads, and the one search ranks them with the rest.
    rows = admin.rdf.sparql(
        f"SELECT ?asset WHERE {{ ?asset <{CATALOG}readsAsset> <urn:medulla:asset:resource:sales> }}"
    )
    assert len(rows) == 3
    found = {asset.strip("<>") for asset, _ in _ranked(admin, "sales")}
    assert {
        "urn:medulla:asset:dashboard:sales",
        "urn:medulla:asset:report:sales",
        "urn:medulla:asset:template:sales",
    } <= found

    # A template with no parameter is refused and writes nothing.
    with pytest.raises(Exception):
        admin.rdf.add_triples(
            turtle=f"{PREFIX}<urn:medulla:asset:template:bare> a cat:ReportTemplate ; cat:hasOwner {owner} .\n"
        )


SECONDS_PER_DAY = 86_400
# The default of `knob catalog.top_queries_window_days`.
TOP_QUERIES_WINDOW_DAYS = 30


def _digest(question: str) -> str:
    return hashlib.sha256(question.encode()).hexdigest()


def _connect(graph: str, context: dict[str, object]) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=context,
    )


@pytest.fixture
def usage_graph():
    """A graph of its own for the usage case, isolated from xdist peers."""

    graph = f"medulla-r1088-{uuid.uuid4().hex}"
    client = _connect(graph, request_context())
    created = False
    try:
        client.tenants.create(graph)
        created = True
        yield client, graph
    finally:
        try:
            if created:
                client.tenants.delete(graph)
        finally:
            client.close()


def _usage(client: SyncMedullaClient, view: str) -> dict[str, dict]:
    rows = client.query.unified([{"UsageScan": {"view": view}}])
    return {row["id"]: row for row in rows}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.8")
def test_tool_call_and_query_provenance_feed_usage_frequency_and_top_queries(
    usage_graph,
) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R108.8: frequency, last use, users, top queries."""

    client, graph = usage_graph
    client.nodes.add("doc-1", {"node_type": "Doc", "title": "orders runbook"})
    client.nodes.add("person-1", {"node_type": "Person", "name": "steward"})

    # Query provenance: the server records each served question it answered.
    asked = "MATCH (:Doc)"
    other = "MATCH (:Person)"
    for _ in range(3):
        client.query.uql(asked)
    client.query.uql(other)

    # Tool provenance: one run used a tool twice inside the window; a third call of the
    # same tool ran before the window and is not counted.
    now = int(time.time())
    client.nodes.add("run-1", {"node_type": "RunTrace"})
    for call, at in [
        ("tc-1", now),
        ("tc-2", now),
        ("tc-old", now - (TOP_QUERIES_WINDOW_DAYS + 10) * SECONDS_PER_DAY),
    ]:
        client.nodes.add(
            call,
            {"node_type": "ToolCall", "tool_name": "graph_query", "valid_from": at},
        )
    client.edges.add("run-1", "tc-1", {"relationship": "USED_TOOL"})
    client.edges.add("run-1", "tc-2", {"relationship": "USED_TOOL"})

    # The top-queries view ranks question digests by use; a row is the digest of what
    # was asked and its counts, never a result row.
    queries = client.query.unified([{"UsageScan": {"view": "queries"}}])
    assert [row["id"] for row in queries] == [_digest(asked), _digest(other)]
    top = queries[0]
    assert top["values"]["uses"] == 3
    assert top["values"]["users"] == 1
    assert now - 60 <= top["values"]["last_used"] <= now + 60
    assert queries[1]["values"]["uses"] == 1
    assert all(not row.get("texts") for row in queries)
    assert all(
        set(row["values"]) == {"uses", "last_used", "users", "runs"} for row in queries
    )

    # The same view in UQL, limited to the most used question.
    answer = client.query.uql("USAGE QUERIES |> LIMIT 1")
    assert answer["kind"] == "rows", answer
    assert [row["id"] for row in answer["rows"]] == [_digest(asked)]

    # Asset usage frequency: reads from query provenance and runs of tools from
    # `ToolCall` / `RunTrace` provenance.
    assets = _usage(client, "assets")
    assert assets["Doc"]["values"]["uses"] == 3
    assert assets["Doc"]["values"]["users"] == 1
    assert assets["Person"]["values"]["uses"] == 1
    assert assets["graph_query"]["values"]["uses"] == 2
    assert assets["graph_query"]["values"]["runs"] == 1
    assert now - 60 <= assets["graph_query"]["values"]["last_used"] <= now + 60
    assert list(assets)[0] == "Doc"

    # Reading the views is not itself counted as a question.
    assert [
        row["id"] for row in client.query.unified([{"UsageScan": {"view": "queries"}}])
    ] == [
        _digest(asked),
        _digest(other),
    ]

    # A carrier for another tenant is refused at the door; the owner's answer stays.
    other_tenant = _connect(graph, {**request_context(), "tenant": "tenant:other"})
    try:
        with pytest.raises(Exception, match="."):
            other_tenant.query.unified([{"UsageScan": {"view": "queries"}}])
    finally:
        other_tenant.close()
    assert _usage(client, "queries")[_digest(asked)]["values"]["uses"] == 3
