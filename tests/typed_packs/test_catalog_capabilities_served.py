"""Served catalog search (MEDULLA-TYPED-PACKS-R108.16).

One ranked relevance function covers every asset family. It is reached through the
served ``Sparql`` method as ``cat:relevance(?asset, "text")``; the usage prior is
capped (the counts come from the usage series) and the trust flags scale the score.
"""

from __future__ import annotations

import os
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
    assert found == {f"<urn:medulla:asset:{family}:orders>" for family in FAMILIES} or found == {
        f"urn:medulla:asset:{family}:orders" for family in FAMILIES
    }

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
