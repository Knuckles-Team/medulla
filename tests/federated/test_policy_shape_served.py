"""MEDULLA-FEDERATED-QUERY-R706.12 served acceptance: a tenant shape policy holds the
result of every query language to the same verdict.

One graph holds three claims. A `filter` shape rejects drafts and a `refuse` shape
rejects secrets, both written as `QueryPolicy` records. The same question runs through
`Method::Uql`, `Method::Sparql` and `Method::Sql`: with only the `filter` shape each
answers the published and the secret claim, and with the `refuse` shape added each
refuses with `POLICY_VIOLATION` and names the policy, never a claim value.
"""

from __future__ import annotations

import pytest
from _sparql_solutions import solution_values

CORE = "https://knuckles-team.github.io/ontology/core#"
SECRET_VALUE = "ssn-078-05-1120"


def _not_status(status: str) -> str:
    """A shape every `:Claim` whose `:status` is `status` fails."""
    return f"""
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix : <{CORE}> .
:Not{status} a sh:NodeShape ;
    sh:targetClass :Claim ;
    sh:not [ sh:property [ sh:path :status ; sh:hasValue "{status}" ] ] .
"""


def _claims(client) -> None:
    for node_id, status in (
        ("claim-ok", "published"),
        ("claim-draft", "draft"),
        ("claim-secret", "secret"),
    ):
        client.nodes.add(
            node_id, {"type": "Claim", "status": status, "ssn": SECRET_VALUE}
        )


def _policy(client, policy_id: str, status: str, effect: str) -> None:
    client.nodes.add(
        policy_id,
        {
            "type": "QueryPolicy",
            "kind": "shape",
            "shape_turtle": _not_status(status),
            "effect": effect,
        },
    )


def _local(value: str) -> str:
    """A node binding without its brackets and namespace."""
    return value.strip("<>").removeprefix(CORE)


def _uql(client) -> list[str]:
    result = client.query.uql("MATCH (:Claim)")
    return sorted(row["id"] for row in result["rows"])


def _sparql(client) -> list[str]:
    rows = solution_values(
        client.rdf.sparql(
            f"SELECT ?s WHERE {{ ?s a <{CORE}Claim> }}",
            base_iri=CORE,
            type_convention="camel",
        )
    )
    return sorted(_local(row["s"]) for row in rows)


def _sql(client) -> list[str]:
    rows = client.query.sql("SELECT id FROM nodes WHERE label = 'Claim'")
    return sorted(row["id"] for row in rows)


LANGUAGES = {"uql": _uql, "sparql": _sparql, "sql": _sql}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R706.12")
def test_filter_and_refuse_shapes_over_uql_and_sparql(clean_graph) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R706.12: with a `filter` shape, UQL, SPARQL and
    SQL return equal filtered rows; with a `refuse` shape added they refuse with the
    same code, naming the policy and no claim value."""
    _claims(clean_graph)

    # Before any policy every language answers every claim.
    everything = ["claim-draft", "claim-ok", "claim-secret"]
    for name, run in LANGUAGES.items():
        assert run(clean_graph) == everything, name

    # A `filter` shape drops the draft claim from every language.
    _policy(clean_graph, "policy:no-drafts", "draft", "filter")
    filtered = ["claim-ok", "claim-secret"]
    for name, run in LANGUAGES.items():
        assert run(clean_graph) == filtered, name

    # A `refuse` shape refuses the whole request in every language, with one code.
    _policy(clean_graph, "policy:no-secrets", "secret", "refuse")
    for name, run in LANGUAGES.items():
        with pytest.raises(Exception, match="POLICY_VIOLATION") as refused:
            run(clean_graph)
        text = str(refused.value)
        assert "policy:no-secrets" in text, name
        assert SECRET_VALUE not in text, name
        assert "claim-secret" not in text, name
