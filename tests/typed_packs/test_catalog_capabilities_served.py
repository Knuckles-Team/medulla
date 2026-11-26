"""Served catalog search (MEDULLA-TYPED-PACKS-R108.16).

One ranked relevance function covers every asset family. It is reached through the
served ``Sparql`` method as ``cat:relevance(?asset, "text")``; the usage prior is
capped (the counts come from the usage series) and the trust flags scale the score.
"""

# ruff: noqa: E501  (long IRI and Turtle literals in served-proof fixtures)

from __future__ import annotations

import hashlib
import os
import time
import uuid

import pytest
import stewardship_support as support
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient

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
            f"<urn:medulla:asset:{family}:orders> a cat:{cls} ; skos:prefLabel "
            f'"orders {family}" .\n'
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

    # An endorsement (cat:flag) raises the rank of the asset it is set on.
    prompt = "urn:medulla:asset:prompt:orders"
    admin.rdf.add_triples(turtle=f'{PREFIX}<{prompt}> cat:flag "endorsement" .\n')
    after_endorse = dict(_ranked(admin, "orders"))
    endorsed = next(k for k in after_endorse if k.strip("<>") == prompt)
    unendorsed = next(k for k in baseline if k.strip("<>") == prompt)
    assert after_endorse[endorsed] > baseline[unendorsed]

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
            f"<urn:medulla:asset:resource:sales> a cat:DataResource ; skos:prefLabel "
            f'"sales resource" ; '
            f'cat:flag "owner_missing" ; '
            f"<https://knuckles-team.github.io/ontology/connector-pack#ofConnector> "
            f"<urn:c:a> ; "
            f"<https://knuckles-team.github.io/ontology/connector-pack#servesClass> "
            f"<urn:class:Sale> .\n"
            f"<urn:medulla:asset:dashboard:sales> a cat:DashboardAsset ; "
            f'skos:prefLabel "sales dashboard" ; '
            f'cat:hasOwner {owner} ; cat:revision "1"^^<{xsd_int}> ; cat:readsAsset '
            f"<urn:medulla:asset:resource:sales> .\n"
            f"<urn:medulla:asset:report:sales> a cat:ReportAsset ; skos:prefLabel "
            f'"sales report" ; '
            f"cat:hasOwner {owner} ; cat:readsAsset "
            f"<urn:medulla:asset:resource:sales> .\n"
            f"<urn:medulla:asset:template:sales> a cat:ReportTemplate ; "
            f'skos:prefLabel "sales template" ; '
            f'cat:hasOwner {owner} ; cat:parameterName "region" ; cat:readsAsset '
            f"<urn:medulla:asset:resource:sales> .\n"
        )
    )
    # Each reporting asset lists what it reads, and the one search ranks them with
    # the rest.
    rows = admin.rdf.sparql(
        f"SELECT ?asset WHERE {{ ?asset <{CATALOG}readsAsset> "
        f"<urn:medulla:asset:resource:sales> }}"
    )
    assert len(rows) == 3
    found = {asset.strip("<>") for asset, _ in _ranked(admin, "sales")}
    assert {
        "urn:medulla:asset:dashboard:sales",
        "urn:medulla:asset:report:sales",
        "urn:medulla:asset:template:sales",
    } <= found

    # A template with no parameter is refused and writes nothing.
    with pytest.raises(EngineResponseError):
        admin.rdf.add_triples(
            turtle=(
                f"{PREFIX}<urn:medulla:asset:template:bare> a cat:ReportTemplate ; "
                f"cat:hasOwner {owner} .\n"
            )
        )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.15")
def test_model_asset_links_registry_ref_schemas_and_training_lineage_without_weights(
    catalog_graph,
):
    # spec: MEDULLA-TYPED-PACKS-R108.15
    admin = catalog_graph
    xsd_any_uri = "http://www.w3.org/2001/XMLSchema#anyURI"
    evaluation = (
        '{"metric":"auc","value":0.91,"dataset":"holdout","evaluated_at":"2026-11-01"}'
    )
    admin.rdf.add_triples(
        turtle=(
            f"{PREFIX}"
            "<urn:medulla:asset:resource:train> a cat:DataResource ; skos:prefLabel "
            '"training data" ; cat:flag "owner_missing" ; '
            "<https://knuckles-team.github.io/ontology/connector-pack#ofConnector> <urn:c:a> ; "
            "<https://knuckles-team.github.io/ontology/connector-pack#servesClass> "
            "<urn:class:Sale> ; cat:lineageTo <urn:medulla:asset:model:churn> .\n"
            "<urn:medulla:asset:model:churn> a cat:ModelAsset ; skos:prefLabel "
            '"churn model" ; cat:flag "owner_missing" ; '
            f'cat:modelRegistryRef "registry://models/churn/3"^^<{xsd_any_uri}> ; '
            'cat:modelInputSchema "{\\"type\\":\\"object\\"}" ; '
            'cat:modelOutputSchema "{\\"type\\":\\"number\\"}" ; '
            "cat:trainedOn <urn:medulla:asset:resource:train> ; "
            f"cat:evaluationResult {_turtle_literal(evaluation)} .\n"
        )
    )
    # The training lineage is the existing lineage property of the dataset.
    reached = {
        row["node"].strip("<>")
        for row in admin.rdf.sparql(
            f"SELECT ?node WHERE {{ <urn:medulla:asset:resource:train> "
            f"<{CATALOG}lineageTo>+ ?node }}"
        )
    }
    assert reached == {"urn:medulla:asset:model:churn"}
    trained = admin.rdf.sparql(
        f"SELECT ?d WHERE {{ <urn:medulla:asset:model:churn> <{CATALOG}trainedOn> ?d }}"
    )
    assert len(trained) == 1

    # A model asset with no registry reference is refused and writes nothing.
    with pytest.raises(EngineResponseError):
        admin.rdf.add_triples(
            turtle=(
                f"{PREFIX}<urn:medulla:asset:model:bare> a cat:ModelAsset ; "
                'skos:prefLabel "bare" ; cat:flag "owner_missing" .\n'
            )
        )
    # An input schema over 65536 bytes is refused.
    oversized = "x" * 65537
    with pytest.raises(EngineResponseError):
        admin.rdf.add_triples(
            turtle=(
                f"{PREFIX}<urn:medulla:asset:model:big> a cat:ModelAsset ; "
                'skos:prefLabel "big" ; cat:flag "owner_missing" ; '
                f'cat:modelRegistryRef "registry://models/big/1"^^<{xsd_any_uri}> ; '
                f'cat:modelInputSchema "{oversized}" .\n'
            )
        )
    # The engine stores references and metadata, never weights: the model node
    # carries no weights property.
    props = {
        row["p"].strip("<>")
        for row in admin.rdf.sparql(
            "SELECT ?p WHERE { <urn:medulla:asset:model:churn> ?p ?o }"
        )
    }
    assert not any("weights" in p for p in props)
    assert admin.rdf.sparql(f"SELECT ?m WHERE {{ ?m a <{CATALOG}ModelAsset> }}")


def _turtle_literal(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.7")
def test_contract_harvest_stores_schema_keys_views_indexes_and_no_rows(catalog_graph):
    # spec: MEDULLA-TYPED-PACKS-R108.7
    admin = catalog_graph
    cp = "https://knuckles-team.github.io/ontology/connector-pack#"
    digest = "cd" * 32
    owl_class = "http://www.w3.org/2002/07/owl#Class"

    def asset(node: str, kind: str, label: str, flags: tuple[str, ...] = ()) -> str:
        flag = "".join(f' ; cat:flag "{value}"' for value in flags)
        return (
            f'<{node}> a cat:{kind} ; skos:prefLabel "{label}"{flag} ; '
            f'<{cp}ofConnector> <urn:harvest:connector> ; <{cp}digest> "{digest}" .\n'
        )

    turtle = (
        f"{PREFIX}<urn:harvest:Order> a <{owl_class}> .\n"
        f"<urn:harvest:connector> a <{cp}Connector> .\n"
        '<urn:harvest:schema:sales> a cat:SchemaAsset ; skos:prefLabel "sales" ; '
        'cat:flag "owner_missing" ; cat:schemaTable <urn:harvest:table:orders> .\n'
        + asset(
            "urn:harvest:table:orders", "DataResource", "orders", ("owner_missing",)
        )
        + f"<urn:harvest:table:orders> <{cp}servesClass> <urn:harvest:Order> ; "
        "cat:hasField <urn:harvest:field:id> , <urn:harvest:field:note> .\n"
        + asset("urn:harvest:field:id", "Field", "id", ("owner_missing",))
        + asset(
            "urn:harvest:field:note",
            "Field",
            "note",
            ("owner_missing", "field_unmapped"),
        )
        + asset(
            "urn:harvest:view:open_orders",
            "ViewAsset",
            "open orders",
            ("owner_missing",),
        )
        + f"<urn:harvest:view:open_orders> <{cp}servesClass> <urn:harvest:Order> .\n"
        '<urn:harvest:key:orders_pk> a cat:KeyAsset ; skos:prefLabel "orders pk" ; '
        'cat:flag "owner_missing" ; cat:keyKind "primary" ; '
        "cat:keyOf <urn:harvest:table:orders> ; cat:keyField <urn:harvest:field:id> .\n"
        '<urn:harvest:index:orders_note> a cat:IndexAsset ; skos:prefLabel "orders note" ; '
        'cat:flag "owner_missing" ; cat:indexOf <urn:harvest:table:orders> ; '
        "cat:indexField <urn:harvest:field:note> .\n"
    )
    admin.rdf.add_triples(turtle=turtle)

    # Schema, table, field, key, view and index are all nodes; a field with no ontology
    # mapping committed flagged.
    for kind, count in (
        ("SchemaAsset", 1),
        ("DataResource", 2),
        ("Field", 2),
        ("KeyAsset", 1),
        ("IndexAsset", 1),
    ):
        rows = admin.rdf.sparql(f"SELECT ?n WHERE {{ ?n a <{CATALOG}{kind}> }}")
        assert len(rows) == count, (kind, rows)
    unmapped = {
        row["n"].strip("<>")
        for row in admin.rdf.sparql(
            f'SELECT ?n WHERE {{ ?n <{CATALOG}flag> "field_unmapped" }}'
        )
    }
    assert unmapped == {"urn:harvest:field:note"}

    # Medulla stores metadata only: no node carries a row or a package payload.
    predicates = {
        row["p"].strip("<>")
        for row in admin.rdf.sparql("SELECT DISTINCT ?p WHERE { ?s ?p ?o }")
    }
    assert not any(p.endswith(("#row", "#rows", "#package")) for p in predicates)

    # A flag outside the closed set is refused and writes nothing.
    with pytest.raises(EngineResponseError):
        admin.rdf.add_triples(
            turtle=(
                f'{PREFIX}<urn:harvest:field:bad> a cat:Field ; cat:flag "mystery" .\n'
            )
        )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.10")
def test_lineage_walks_source_operation_workflow_dashboard_agent_and_model(
    catalog_graph,
):
    # spec: MEDULLA-TYPED-PACKS-R108.10
    admin = catalog_graph
    chain = [
        ("urn:lineage:source", "connector", "urn:lineage:operation"),
        ("urn:lineage:operation", "transform", "urn:lineage:workflow"),
        ("urn:lineage:workflow", "ingest", "urn:lineage:dashboard"),
        ("urn:lineage:dashboard", "resource", "urn:lineage:agent"),
        ("urn:lineage:agent", "resource", "urn:lineage:model"),
    ]
    turtle = PREFIX
    for n, (upstream, hop, downstream) in enumerate(chain):
        turtle += (
            f"<{upstream}> cat:lineageTo <{downstream}> .\n"
            f"<urn:lineage:edge:{n}> a cat:LineageEdge ; cat:from <{upstream}> ;"
            f' cat:to <{downstream}> ; cat:hop "{hop}" .\n'
        )
    admin.rdf.add_triples(turtle=turtle)

    # One property path walks the whole platform from the source to the model.
    reached = {
        row["node"].strip("<>")
        for row in admin.rdf.sparql(
            f"SELECT ?node WHERE {{ <urn:lineage:source> <{CATALOG}lineageTo>+ ?node }}"
        )
    }
    assert reached == {downstream for _, _, downstream in chain}

    # The hop type reads from the lineage edge node and stays in the closed set.
    hops = {
        (row["from"].strip("<>"), row["to"].strip("<>")): row["hop"].strip('"')
        for row in admin.rdf.sparql(
            f"SELECT ?from ?to ?hop WHERE {{ ?e a <{CATALOG}LineageEdge> ; "
            f"<{CATALOG}from> ?from ; <{CATALOG}to> ?to ; <{CATALOG}hop> ?hop }}"
        )
    }
    assert hops == {(upstream, downstream): hop for upstream, hop, downstream in chain}

    # A hop outside the closed set is refused and writes nothing.
    with pytest.raises(Exception, match="hop|sh:in|VALUE_CONSTRAINT"):
        admin.rdf.add_triples(
            turtle=(
                f"{PREFIX}<urn:lineage:edge:bad> a cat:LineageEdge ; cat:from <urn:lineage:a> ;"
                ' cat:to <urn:lineage:b> ; cat:hop "teleport" .\n'
                "<urn:lineage:a> cat:lineageTo <urn:lineage:b> .\n"
            )
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


def _classification(client, graph: str, op: dict) -> dict:
    import msgpack

    raw = client._send("ClassificationClaim", {"op": op}, graph)
    return (
        msgpack.unpackb(raw, raw=False) if isinstance(raw, (bytes, bytearray)) else raw
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.9")
def test_sensitivity_propagates_downstream_as_proposal_and_asset_names_its_policy(
    catalog_graph,
):
    # spec: MEDULLA-TYPED-PACKS-R108.9
    import msgpack

    admin = catalog_graph
    graph = admin._client._graph_name
    steward = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(roles=["steward"]),
    )

    def node(node_id: str, properties: dict) -> None:
        steward._send(
            "AddNode",
            {
                "node_id": node_id,
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
            graph,
        )

    try:
        node("marking:pii", {"type": "Marking"})
        for asset in ("col:r1089.src", "view:r1089.view"):
            node(asset, {"type": "Column"})
        steward._send(
            "AddEdge",
            {
                "source_id": "col:r1089.src",
                "target_id": "view:r1089.view",
                "properties_msgpack": msgpack.packb(
                    {"relationship": "cat:lineageTo"}, use_bin_type=True
                ),
            },
            graph,
        )
        claim = {
            "asset": "col:r1089.src",
            "categories": ["pii"],
            "provenance": {"kind": "manual"},
            "state": "proposed",
            "evidence": [],
        }
        _classification(steward, graph, {"op": "propose", "claim": claim})
        promoted = _classification(
            steward,
            graph,
            {
                "op": "promote",
                "claim_id": "classification_claim:col:r1089.src:pii",
                "evidence": ["review:1"],
            },
        )
        # The derived asset receives a proposal for a steward to promote, not a stamp.
        derived = promoted["propagated"]
        assert [r["claim"]["asset"] for r in derived] == ["view:r1089.view"]
        assert derived[0]["claim"]["state"] == "proposed"
    finally:
        steward.close()

    # An asset names its governing policy by IRI; the catalog keeps the link only.
    asset = "urn:medulla:asset:product:governed"
    policy = "urn:medulla:policy:orders-mask"
    admin.rdf.add_triples(
        turtle=(
            f"{PREFIX}<{asset}> a cat:Product ; cat:hasOwner <urn:owner:a> ; "
            f"cat:governedBy <{policy}> .\n"
        )
    )
    rows = admin.rdf.sparql(f"SELECT ?o WHERE {{ <{asset}> <{CATALOG}governedBy> ?o }}")
    assert [row["o"].strip("<>") for row in rows] == [policy]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.13")
def test_article_attaches_to_asset_and_domain_and_is_found_by_text_and_by_asset(
    catalog_graph,
):
    """Verifies MEDULLA-TYPED-PACKS-R108.13: an article moves through the one status path."""
    import msgpack
    import stewardship_support as support
    from stewardship_support import REVIEWER

    admin = catalog_graph
    graph = admin._client._graph_name
    asset = "urn:medulla:asset:resource:invoices"
    domain = "urn:medulla:asset:domain:finance"
    article = "article:onboarding"

    def article_turtle(version: int, status: str) -> str:
        return (
            f'{PREFIX}<{asset}> a cat:DataResource ; skos:prefLabel "invoices" .\n'
            f'<{domain}> a cat:BusinessDomain ; skos:prefLabel "finance" .\n'
            f'<{article}> a cat:Article ; skos:prefLabel "Invoice onboarding" ;\n'
            f'  cat:author "usr:writer" ; cat:articleVersion {version} ;\n'
            f'  cat:articleStatus "{status}" ;\n'
            f"  cat:aboutAsset <{asset}> ; cat:inBusinessDomain <{domain}> .\n"
        )

    admin.rdf.add_triples(turtle=article_turtle(2, "draft"))
    # A draft article is returned by no search, by its text or by the asset it describes.
    assert not any(article in asset_id for asset_id, _ in _ranked(admin, "onboarding"))
    assert not any(article in asset_id for asset_id, _ in _ranked(admin, "invoices"))
    # A rewrite at a lower version is refused.
    with pytest.raises(Exception, match="ARTICLE_VERSION_INVALID"):
        admin.rdf.add_triples(turtle=article_turtle(1, "draft"))

    def decode(value):
        return (
            msgpack.unpackb(value, raw=False)
            if isinstance(value, (bytes, bytearray))
            else value
        )

    def move(client, event, change_request=None):
        op = {"op": "transition", "iri": f"<{article}>", "event": event}
        if change_request:
            op["change_request"] = change_request
        return decode(client._send("Glossary", {"op": op}, graph))

    assert move(admin, "submit")["status"] == "in_review"
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        support.register_agents(graph, REVIEWER)
        request_id = support.approved_request(admin, reviewer, graph, f"<{article}>")
    finally:
        reviewer.close()
    assert move(admin, "approve", request_id)["status"] == "approved"
    moves = decode(
        admin._send(
            "GetNodesByLabel",
            {"label": "cat:GlossaryStatusMove", "after": None, "limit": 10},
            graph,
        )
    )
    assert {props["subject_kind"] for _, props in moves} == {"article"}
    assert len(moves) == 2

    # The approved article is found by its text and by the asset it describes; a draft is not.
    assert any(article in asset_id for asset_id, _ in _ranked(admin, "onboarding"))
    assert any(article in asset_id for asset_id, _ in _ranked(admin, "invoices"))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.12")
def test_annotations_attach_to_any_asset_and_trust_flags_reuse_the_steward_path(
    catalog_graph,
):
    # spec: MEDULLA-TYPED-PACKS-R108.12
    admin = catalog_graph
    xsd = "http://www.w3.org/2001/XMLSchema#"
    asset = "urn:medulla:asset:resource:orders"
    note = "urn:annotation:orders:1"
    asset_turtle = (
        f'{PREFIX}<{asset}> a cat:DataResource ; skos:prefLabel "orders ledger" ;'
        " cat:hasOwner <urn:principal:data-team> ;"
        " <https://knuckles-team.github.io/ontology/connector-pack#ofConnector> <urn:c:a> ;"
        " <https://knuckles-team.github.io/ontology/connector-pack#servesClass> <urn:class:Order> ;"
        f' <https://knuckles-team.github.io/ontology/connector-pack#digest> "{"ab" * 32}" .\n'
    )
    admin.rdf.add_triples(turtle=asset_turtle)

    def annotation(iri: str, kind: str, extra: str) -> str:
        return (
            f"{PREFIX}<{iri}> a cat:Annotation ; cat:annotates <{asset}> ;"
            f' cat:annotationKind "{kind}" ; cat:annotationAuthor "agent-a" ;'
            f' cat:annotatedAtMs "1700000000000"^^<{xsd}nonNegativeInteger> ;'
            f' cat:annotationRevision "1"^^<{xsd}nonNegativeInteger> ; {extra} .\n'
        )

    # A comment, a tag, a rating and a reply attach to the asset.
    admin.rdf.add_triples(
        turtle=annotation(note, "comment", 'cat:annotationText "check the totals"')
    )
    admin.rdf.add_triples(
        turtle=annotation(
            "urn:annotation:orders:2", "tag", 'cat:annotationText "finance"'
        )
    )
    admin.rdf.add_triples(
        turtle=annotation(
            "urn:annotation:orders:3", "rating", f'cat:rating "5"^^<{xsd}integer>'
        )
    )
    admin.rdf.add_triples(
        turtle=annotation(
            "urn:annotation:orders:4",
            "comment",
            f'cat:annotationText "done" ; cat:inReplyTo <{note}>',
        )
    )
    rows = admin.rdf.sparql(
        f"PREFIX cat: <{CATALOG}> SELECT ?a WHERE {{ ?a cat:annotates <{asset}> }}"
    )
    assert len(rows) == 4

    # A rating outside 1 to 5 is refused and writes nothing.
    with pytest.raises(
        Exception, match="rating|minInclusive|maxInclusive|VALUE_CONSTRAINT"
    ):
        admin.rdf.add_triples(
            turtle=annotation(
                "urn:annotation:orders:5", "rating", f'cat:rating "6"^^<{xsd}integer>'
            )
        )

    # A trust flag reuses the steward path: one `SetTrustFlag` writes it and ranking reads it.
    graph = admin._graph_name
    before = dict(_ranked(admin, "orders"))
    key = next(k for k in before if k.strip("<>") == asset)
    support.stewardship(
        admin,
        graph,
        {
            "op": "set_trust_flag",
            "asset": asset,
            "flag": "deprecation",
            "reason": "superseded",
        },
    )
    after = dict(_ranked(admin, "orders"))
    assert after[next(k for k in after if k.strip("<>") == asset)] < before[key]

    # A thread on a deleted asset stays readable by revision: the annotation nodes are
    # independent nodes, so deleting the asset leaves every comment, tag and rating
    # readable with its author and revision.
    admin.rdf.remove_triples(turtle=asset_turtle)
    thread = admin.rdf.sparql(
        f"PREFIX cat: <{CATALOG}> "
        "SELECT ?a ?revision WHERE { ?a a cat:Annotation ; cat:annotationRevision ?revision }"
    )
    assert len(thread) == 4
    assert all(str(row["revision"]).strip('"').startswith("1") for row in thread)


def _read(client, graph: str, op: dict):
    return support.decode(client._send("CatalogRead", {"op": op}, graph))


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.17")
def test_registered_catalog_queries_browse_search_and_detail_hide_unreadable_assets():
    # spec: MEDULLA-TYPED-PACKS-R108.17
    graph = f"medulla-r10817-{uuid.uuid4().hex}"
    owner_id, stranger_id = "agent-catalog-reader", "agent-catalog-stranger"
    admin = support.connect()
    created = False
    owner = stranger = None
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        support.register_agents(graph, owner_id, stranger_id)
        owner = support.connect(graph, agent_id=owner_id)
        stranger = support.connect(graph, agent_id=stranger_id)
        xsd = "http://www.w3.org/2001/XMLSchema#"
        tool = "urn:medulla:asset:tool:orders"
        skill = "urn:medulla:asset:skill:orders"
        prompt = "urn:medulla:asset:prompt:refunds"
        owner.rdf.add_triples(
            turtle=(
                f'{PREFIX}<{tool}> a cat:ToolAsset ; skos:prefLabel "orders tool" ;'
                " cat:hasOwner <urn:principal:data-team> .\n"
                f'<{skill}> a cat:SkillAsset ; skos:prefLabel "orders skill" ;'
                " cat:hasOwner <urn:principal:ops-team> .\n"
                f'<{prompt}> a cat:PromptAsset ; skos:prefLabel "refunds prompt" ;'
                " cat:hasOwner <urn:principal:data-team> .\n"
                f"<urn:annotation:orders:1> a cat:Annotation ; cat:annotates <{tool}> ;"
                ' cat:annotationKind "rating" ; cat:annotationAuthor "agent-a" ;'
                f' cat:annotatedAtMs "1700000000000"^^<{xsd}nonNegativeInteger> ;'
                f' cat:annotationRevision "1"^^<{xsd}nonNegativeInteger> ;'
                f' cat:rating "4"^^<{xsd}integer> .\n'
                f"<urn:annotation:orders:2> a cat:Annotation ; cat:annotates <{tool}> ;"
                ' cat:annotationKind "comment" ; cat:annotationAuthor "agent-a" ;'
                f' cat:annotatedAtMs "1700000000001"^^<{xsd}nonNegativeInteger> ;'
                f' cat:annotationRevision "1"^^<{xsd}nonNegativeInteger> ;'
                ' cat:annotationText "check the totals" .\n'
            )
        )

        # Browse: the kind counts and the rows come back in one result, filtered by kind/owner.
        browsed = _read(owner, graph, {"op": "browse"})
        counts = {entry["kind"]: entry["count"] for entry in browsed["kind_counts"]}
        assert (
            counts["ToolAsset"] == 1
            and counts["SkillAsset"] == 1
            and counts["PromptAsset"] == 1
        )
        assert {row["asset"] for row in browsed["rows"]} >= {tool, skill, prompt}
        only_tools = _read(owner, graph, {"op": "browse", "kind": "ToolAsset"})
        assert [row["asset"] for row in only_tools["rows"]] == [tool]
        data_team = _read(
            owner,
            graph,
            {"op": "browse", "owner": "urn:principal:data-team", "kind": "PromptAsset"},
        )
        assert [row["asset"] for row in data_team["rows"]] == [prompt]

        # Paging: a page of one carries the cursor of the next page.
        first = _read(owner, graph, {"op": "browse", "limit": 1})
        assert len(first["rows"]) == 1 and first["next_cursor"]
        second = _read(
            owner, graph, {"op": "browse", "limit": 1, "cursor": first["next_cursor"]}
        )
        assert second["rows"][0]["asset"] != first["rows"][0]["asset"]

        # Search: ranked rows, narrowed by the kinds list.
        found = _read(owner, graph, {"op": "search", "text": "orders"})
        assert {row["asset"] for row in found["rows"]} == {tool, skill}
        assert all(row["score"] > 0 for row in found["rows"])
        narrowed = _read(
            owner, graph, {"op": "search", "text": "orders", "kinds": ["SkillAsset"]}
        )
        assert [row["asset"] for row in narrowed["rows"]] == [skill]

        # Detail: owners, rating count and mean, and the comment.
        detail = _read(owner, graph, {"op": "detail", "asset_id": tool})["detail"]
        assert detail["kind"] == "ToolAsset" and detail["owners"] == [
            "urn:principal:data-team"
        ]
        assert detail["rating_count"] == 1 and detail["rating_mean"] == 4.0
        assert [c["text"] for c in detail["comments"]] == ["check the totals"]

        # A caller who may read none of it sees no row and no count, and an asset it cannot
        # read answers like an unknown one.
        hidden = _read(stranger, graph, {"op": "browse"})
        assert hidden["rows"] == [] and all(
            e["count"] == 0 for e in hidden["kind_counts"]
        )
        assert _read(stranger, graph, {"op": "search", "text": "orders"})["rows"] == []
        with pytest.raises(Exception, match="ASSET_NOT_FOUND") as unreadable:
            _read(stranger, graph, {"op": "detail", "asset_id": tool})
        with pytest.raises(Exception, match="ASSET_NOT_FOUND") as unknown:
            _read(
                owner,
                graph,
                {"op": "detail", "asset_id": "urn:medulla:asset:tool:none"},
            )
        assert (
            str(unreadable.value).split("ASSET_NOT_FOUND")[1]
            == str(unknown.value).split("ASSET_NOT_FOUND")[1]
        )

        # The refusals: unknown kind, bad cursor, page size out of bounds, text over the knob.
        for op in (
            {"op": "browse", "kind": "NoSuchKind"},
            {"op": "browse", "cursor": "not-a-cursor"},
            {"op": "browse", "limit": 0},
            {"op": "search", "text": "orders", "kinds": ["NoSuchKind"]},
        ):
            with pytest.raises(Exception, match="INVALID_ARGUMENT"):
                _read(owner, graph, op)
        with pytest.raises(Exception, match="CATALOG_QUERY_TOO_LONG"):
            _read(owner, graph, {"op": "search", "text": "x" * 1001})
    finally:
        for client in (owner, stranger):
            if client is not None:
                client.close()
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.11")
def test_metric_formula_binds_terms_and_unknown_term_refuses(catalog_graph):
    """Verifies MEDULLA-TYPED-PACKS-R108.11: a metric formula names glossary terms by label."""
    admin = catalog_graph
    domain = "urn:medulla:asset:domain:finance"
    term = "urn:medulla:term:revenue"

    def metric(node: str, formula: str, extra: str = "") -> str:
        escaped = formula.replace("\\", "\\\\").replace('"', '\\"')
        return (
            f'{PREFIX}<{domain}> a cat:BusinessDomain ; skos:prefLabel "finance" .\n'
            f'<{node}> a cat:Metric ; skos:prefLabel "{node}" ;\n'
            f'  cat:metricName "Net revenue" ; cat:metricDomain <{domain}> ;\n'
            f'  cat:metricUnit "USD" ; cat:metricFormula "{escaped}" .\n{extra}'
        )

    term_turtle = (
        f'{PREFIX}<{term}> a cat:CatalogGlossaryTerm ; skos:prefLabel "Revenue" ;\n'
        f"  cat:definesClass cat:Asset .\n"
    )

    # An unknown term refuses, and the write changes nothing: the later write that
    # introduces the term and the metric together is the first to land.
    with pytest.raises(Exception, match="METRIC_TERM_UNRESOLVED"):
        admin.rdf.add_triples(turtle=metric("urn:metric:a", "{term:Revenue} - 1"))
    # A term never resolves by node IRI.
    with pytest.raises(Exception, match="METRIC_TERM_UNRESOLVED"):
        admin.rdf.add_triples(turtle=metric("urn:metric:b", f"{{term:{term}}}"))
    # Another prefix, an unclosed brace and a property that is not an absolute IRI refuse.
    for bad in (
        "{column:revenue}",
        "{term:Revenue",
        "{prop:has space}",
        "{prop:relative}",
    ):
        with pytest.raises(Exception, match="METRIC_TERM_UNRESOLVED"):
            admin.rdf.add_triples(turtle=metric("urn:metric:c", bad))

    # A term and its metric may arrive together; labels are trimmed and case-sensitive.
    admin.rdf.add_triples(
        turtle=metric(
            "urn:metric:both",
            "{term: Revenue } / {prop:http://schema.org/price}",
            term_turtle,
        )
    )
    # A later metric resolves the stored term; a different case does not.
    admin.rdf.add_triples(turtle=metric("urn:metric:later", "{term:Revenue} * 2"))
    with pytest.raises(Exception, match="METRIC_TERM_UNRESOLVED"):
        admin.rdf.add_triples(turtle=metric("urn:metric:case", "{term:revenue}"))
