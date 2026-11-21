"""Served catalog asset write guard (MEDULLA-TYPED-PACKS-R108.4).

Writes catalog triples through the real served ``AddTriples`` surface. A node
typed with a ``cat:`` class that is not under ``cat:Asset`` is refused with
``ASSET_CLASS_ORPHAN``. A node with no owner commits flagged ``owner_missing``.
A person field the writer holds no marking for is refused with ``FIELD_MARKED``
and the same field commits for a writer whose scopes carry ``marking:pii``. A
refused write changes nothing.
"""

# ruff: noqa: E501

from __future__ import annotations

import os
import uuid

import pytest
import stewardship_support as support
from conftest import TEST_TENANT, request_context

from medulla.client import SyncMedullaClient

CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
PREFIX = f"@prefix cat: <{CATALOG}> .\n"
SHAPES = (
    "@prefix sh: <http://www.w3.org/ns/shacl#> .\n"
    "@prefix ex: <http://example.test/r1084#> .\n"
    "ex:UnusedShape a sh:NodeShape ; sh:targetClass ex:UnusedClass .\n"
)


def _connect(graph: str, scopes: list[str] | None) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(scopes=scopes),
    )


@pytest.fixture
def catalog_graph():
    """Give this served test a graph isolated from xdist peers."""

    graph = f"medulla-r1084-{uuid.uuid4().hex}"
    admin = _connect(graph, None)
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        admin.rdf.icv_configure(SHAPES)
        yield graph, admin
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


def _rows(client, subject: str, predicate: str) -> list[str]:
    rows = client.rdf.sparql(f"SELECT ?o WHERE {{ <{subject}> <{predicate}> ?o }}")
    return [row["o"] for row in rows]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.4")
def test_orphan_class_missing_owner_and_marked_field_are_handled(catalog_graph):
    # spec: MEDULLA-TYPED-PACKS-R108.4
    graph, admin = catalog_graph
    rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"

    # An orphan class is refused with its code and nothing is written.
    orphan = "urn:medulla:asset:product:orphan"
    with pytest.raises(Exception, match="ASSET_CLASS_ORPHAN"):
        admin.rdf.add_triples(turtle=f"{PREFIX}<{orphan}> a cat:Widget .\n")
    assert _rows(admin, orphan, rdf_type) == []

    # A node with no owner commits flagged owner_missing.
    ownerless = "urn:medulla:asset:product:ownerless"
    report = admin.rdf.add_triples(turtle=f"{PREFIX}<{ownerless}> a cat:Product .\n")
    assert report["triples"] >= 2
    assert _rows(admin, ownerless, f"{CATALOG}flag") == ["owner_missing"]

    # A node that names an owner is not flagged.
    owned = "urn:medulla:asset:product:owned"
    admin.rdf.add_triples(
        turtle=(
            f"{PREFIX}<{owned}> a cat:Product ; "
            "cat:hasOwner <urn:principal:data-team> .\n"
        )
    )
    assert _rows(admin, owned, f"{CATALOG}flag") == []

    # A marked person field is refused for a writer that holds no marking, even
    # one holding the wildcard scope, and the person is not written.
    person = "urn:medulla:asset:person:alice"
    email = f'{PREFIX}<{person}> a cat:PersonAsset ; cat:email "alice@example.test" .\n'
    with pytest.raises(Exception, match="FIELD_MARKED"):
        admin.rdf.add_triples(turtle=email)
    assert _rows(admin, person, f"{CATALOG}email") == []

    # The same write commits for a writer whose scopes carry the marking.
    cleared = _connect(graph, ["kg:read", "kg:write", "marking:pii"])
    try:
        cleared.rdf.add_triples(turtle=email)
    finally:
        cleared.close()
    assert _rows(admin, person, f"{CATALOG}email") == ["alice@example.test"]


RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
SOURCE_RECORD = f"{CATALOG}sourceRecord"


def _digest(seed: str) -> str:
    return "sha256:" + seed * 64


def _dependency(component_id: str, kind: str, seed: str) -> dict:
    return {
        "component_id": component_id,
        "kind": kind,
        "definition_digest": _digest(seed),
    }


def _agent_draft(agent_id: str) -> dict:
    return {
        "agent_id": agent_id,
        "package_id": "package:catalog-emit",
        "version": "1.0.0",
        "role": "role:researcher",
        "role_digest": _digest("1"),
        "system_prompt": _dependency("prompt:catalog-emit", "system_prompt", "2"),
        "tools": [_dependency("tool:search", "tool", "3")],
        "skills": [_dependency("skill:research", "skill", "4")],
        "model_profile": _dependency("model-profile:default", "model_profile", "5"),
        "model_identity": "model:example",
        "ontologies": [_dependency("ontology:core", "ontology", "6")],
        "tenant_id": TEST_TENANT,
        "actor_scope": "agent-builder",
        "purpose_id": "agent-construction",
        "policy_digest": _digest("7"),
        "source_revision": "source-revision:1",
        "source_revision_digest": _digest("8"),
    }


def _publish_context() -> dict:
    """The mutation context; the engine overwrites everything but the tenant."""
    return {
        "request_id": 1,
        "principal": "principal:sha256:" + "a" * 64,
        "caller_principal": "principal:sha256:" + "b" * 64,
        "attempt_nonce": "09" * 32,
        "tenant_id": TEST_TENANT,
        "actor_scope": "agent-builder",
        "purpose_id": "agent-construction",
        "policy_revision": "policy-v1",
        "policy_digest": _digest("7"),
        "policy_decision_id": "decision:1",
        "idempotency_key": "request:1",
        "expected_revision": 0,
        "created_at_ms": 10,
    }


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.3")
def test_publish_register_and_create_reach_the_one_emitter(catalog_graph):
    """Verifies MEDULLA-TYPED-PACKS-R108.3: each engine-held record is cataloged by `emit_asset`."""
    graph, admin = catalog_graph

    # An Agent Library publish catalogs the published revision on the request graph.
    agent_id = f"agent:catalog-emit-{uuid.uuid4().hex[:8]}"
    published = support.decode(
        admin._send(
            "AgentLibrary",
            {
                "op": {
                    "operation": "publish",
                    "request": {
                        "context": _publish_context(),
                        "entry": _agent_draft(agent_id),
                    },
                }
            },
            graph,
        )
    )
    revision = published["entry"]["entry_revision"]
    agent_node = (
        f"urn:medulla:asset:agent-version:{agent_id.replace(':', '%3A')}%40{revision}"
    )
    assert _rows(admin, agent_node, RDF_TYPE) == [f"{CATALOG}AgentVersion"]
    assert _rows(admin, agent_node, SOURCE_RECORD) == [
        f"agent-library:{agent_id}:{revision}"
    ]

    # A workflow registration catalogs the definition.
    identity = {
        "workflow_id": f"wf-emit-{uuid.uuid4().hex[:8]}",
        "version": "1.0.0",
        "content_digest": _digest("a"),
    }
    admin._send(
        "WorkflowRegistry",
        {
            "op": {
                "op": "register",
                "registration": {
                    "identity": identity,
                    "owner_scope": "",
                    "lifecycle": "active",
                },
            }
        },
        graph,
    )
    workflow_id = f"workflow:{identity['workflow_id']}:{identity['version']}:{identity['content_digest']}"
    workflow_node = "urn:medulla:asset:workflow:" + workflow_id.replace(":", "%3A")
    assert _rows(admin, workflow_node, RDF_TYPE) == [
        f"{CATALOG}WorkflowDefinitionAsset"
    ]
    assert _rows(admin, workflow_node, SOURCE_RECORD) == [workflow_id]

    # A graph creation catalogs the new graph, in that graph.
    created = f"medulla-r1083-{uuid.uuid4().hex}"
    admin.tenants.create(created, graph_type="Global")
    try:
        creator = _connect(created, None)
        try:
            graph_node = f"urn:medulla:asset:graph:{created}"
            assert _rows(creator, graph_node, RDF_TYPE) == [f"{CATALOG}KnowledgeGraph"]
            assert _rows(creator, graph_node, SOURCE_RECORD) == [f"graph:{created}"]
        finally:
            creator.close()
    finally:
        admin.tenants.delete(created)
