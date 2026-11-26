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

import hashlib
import os
import pathlib
import time
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


def _publish_agent(admin, graph: str, agent_id: str, base: int, version: str) -> dict:
    context = _publish_context()
    context["expected_revision"] = base
    context["idempotency_key"] = f"request:{agent_id}:{base}"
    context["attempt_nonce"] = f"{base + 1:02x}" * 32
    draft = _agent_draft(agent_id)
    draft["version"] = version
    return support.decode(
        admin._send(
            "AgentLibrary",
            {
                "op": {
                    "operation": "publish",
                    "request": {"context": context, "entry": draft},
                }
            },
            graph,
        )
    )


def _node_id(agent_id: str, revision: int) -> str:
    return (
        f"urn:medulla:asset:agent-version:{agent_id.replace(':', '%3A')}%40{revision}"
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.5")
def test_node_carries_source_owner_steward_revision_and_history(catalog_graph):
    """Verifies MEDULLA-TYPED-PACKS-R108.5: a catalog node carries its evidence."""
    graph, admin = catalog_graph
    agent_id = f"agent:catalog-evidence-{uuid.uuid4().hex[:8]}"
    owner = support.principal(None)
    steward = "urn:medulla:principal:data-steward"

    # The stewardship store names an owner and a steward of the node before it is written.
    first_node = _node_id(agent_id, 1)
    for role, who in (("owner", owner), ("steward", steward)):
        support.stewardship(
            admin,
            graph,
            {
                "op": "assign",
                "assignment": {
                    "scope": {"kind": "asset", "id": first_node},
                    "role": role,
                    "principal": who,
                },
            },
        )
    before_ms = int(time.time() * 1000) - 60_000
    _publish_agent(admin, graph, agent_id, 0, "1.0.0")

    # The node carries its source record, owner, steward, graph revision and last change.
    assert _rows(admin, first_node, SOURCE_RECORD) == [f"agent-library:{agent_id}:1"]
    assert _rows(admin, first_node, f"{CATALOG}hasOwner") == [
        f"urn:medulla:principal:{owner}"
    ]
    assert _rows(admin, first_node, f"{CATALOG}hasSteward") == [steward]
    assert not _rows(admin, first_node, f"{CATALOG}flag")
    revisions = _rows(admin, first_node, f"{CATALOG}graphRevision")
    assert len(revisions) == 1 and int(_integer(revisions[0])) >= 1
    changed = _rows(admin, first_node, f"{CATALOG}lastChangedMs")
    assert len(changed) == 1 and int(_integer(changed[0])) > before_ms

    # Version history reads from the supersession chain: each published revision is its own
    # node, and the second names its own source record.
    _publish_agent(admin, graph, agent_id, 1, "1.1.0")
    second_node = _node_id(agent_id, 2)
    assert _rows(admin, second_node, SOURCE_RECORD) == [f"agent-library:{agent_id}:2"]
    versions = admin.rdf.sparql(
        f"SELECT ?n WHERE {{ ?n <{RDF_TYPE}> <{CATALOG}AgentVersion> ; "
        f'<{SOURCE_RECORD}> ?s . FILTER(CONTAINS(?s, "{agent_id}:")) }}'
    )
    assert len(versions) == 2


def _integer(term: str) -> str:
    """The lexical value of a typed integer term as the served SPARQL returns it."""
    return term.strip('"<>').split('"')[0].split("^^")[0].strip('"')


FIXTURES = pathlib.Path(__file__).resolve().parents[1] / "fixtures" / "catalog"
ASSET = f"{CATALOG}Asset"


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.6")
def test_one_query_returns_agents_business_workflows_graphs_and_connector_assets(
    catalog_graph,
):
    """Verifies MEDULLA-TYPED-PACKS-R108.6: one query on `cat:Asset` returns every family."""
    graph, admin = catalog_graph

    # An agent publish, a workflow registration and a graph creation reach the one emitter.
    agent_id = f"agent:catalog-journey-{uuid.uuid4().hex[:8]}"
    _publish_agent(admin, graph, agent_id, 0, "1.0.0")
    identity = {
        "workflow_id": f"wf-journey-{uuid.uuid4().hex[:8]}",
        "version": "1.0.0",
        "content_digest": _digest("b"),
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
    created = f"medulla-r1086-{uuid.uuid4().hex}"
    admin.tenants.create(created, graph_type="Global")
    try:
        # The recorded connector onboarding batch lands through the served AddTriples.
        batch = (FIXTURES / "onboarding_batch.nt").read_bytes()
        assert (
            hashlib.sha256(batch).hexdigest()
            == (FIXTURES / "onboarding_batch.sha256").read_text().strip()
        )
        admin.rdf.add_triples(turtle=batch.decode())

        # One query on the parent class returns every family and a connector asset.
        rows = admin.rdf.sparql(
            f"SELECT ?asset ?class WHERE {{ ?asset a ?class . ?asset <{SOURCE_RECORD}> ?s }}"
        )
        classes = {row["class"].strip("<>") for row in rows}
        assert f"{CATALOG}AgentVersion" in classes
        assert f"{CATALOG}WorkflowDefinitionAsset" in classes
        all_assets = {
            row["a"].strip("<>")
            for row in admin.rdf.sparql(f"SELECT ?a WHERE {{ ?a a <{ASSET}> }}")
        }
        assert _node_id(agent_id, 1) in all_assets
        assert any(a.startswith("urn:medulla:asset:workflow:") for a in all_assets)
        assert "urn:medulla:r1066:tool-list-orders" in all_assets
        assert "urn:medulla:r1066:resource-orders" in all_assets

        # The graph the creator made is cataloged in that graph.
        creator = _connect(created, None)
        try:
            node = f"urn:medulla:asset:graph:{created}"
            assert _rows(creator, node, RDF_TYPE) == [f"{CATALOG}KnowledgeGraph"]
        finally:
            creator.close()
    finally:
        admin.tenants.delete(created)
