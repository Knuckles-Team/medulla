"""Served connector catalog write path (MEDULLA-TYPED-PACKS-R106.3).

A batch of catalog nodes goes through the real served ``AddTriples`` surface, which
runs the catalog shapes as the write-time guard. A conforming batch is stored and
found by served UQL; the same batch without ``cp:digest`` is refused and the store
is unchanged.
"""

from __future__ import annotations

import os
import pathlib
import uuid

import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
CONNECTOR_PACK = "https://knuckles-team.github.io/ontology/connector-pack#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
EXAMPLE = "urn:medulla:r1063:"
CONNECTOR = f"{EXAMPLE}connector"
TOOL = f"{EXAMPLE}tool-read"
RESOURCE = f"{EXAMPLE}resource-customers"
OWNER = f"{EXAMPLE}principal-data-team"
DIGEST = "ab" * 32
SHAPES = (
    pathlib.Path(__file__).resolve().parents[2]
    / "crates/medulla-core/ontology/metadata_catalog-v1.shapes.ttl"
).read_text()


def _triple(subject: str, predicate: str, obj: str, *, literal: bool = False) -> str:
    value = f'"{obj}"' if literal else f"<{obj}>"
    return f"<{subject}> <{predicate}> {value} .\n"


OWL_CLASS = "http://www.w3.org/2002/07/owl#Class"


def _batch(
    *,
    with_digest: bool,
    declare_class: bool = True,
    resource_connector: bool = True,
    pair_edge: bool = True,
    serves: str = f"{EXAMPLE}Customer",
) -> str:
    lines = [
        _triple(CONNECTOR, RDF_TYPE, f"{CONNECTOR_PACK}Connector"),
        _triple(TOOL, RDF_TYPE, f"{CATALOG}ToolAsset"),
        _triple(TOOL, f"{CONNECTOR_PACK}ofConnector", CONNECTOR),
        _triple(RESOURCE, RDF_TYPE, f"{CATALOG}DataResource"),
        _triple(RESOURCE, f"{CONNECTOR_PACK}servesClass", serves),
        _triple(f"{EXAMPLE}edge", RDF_TYPE, f"{CATALOG}LineageEdge"),
        _triple(f"{EXAMPLE}edge", f"{CATALOG}from", TOOL),
        _triple(f"{EXAMPLE}edge", f"{CATALOG}to", RESOURCE),
        _triple(f"{EXAMPLE}edge", f"{CATALOG}hop", "resource", literal=True),
    ]
    if declare_class:
        lines.append(_triple(f"{EXAMPLE}Customer", RDF_TYPE, OWL_CLASS))
    if resource_connector:
        lines.append(_triple(RESOURCE, f"{CONNECTOR_PACK}ofConnector", CONNECTOR))
    if pair_edge:
        lines.append(_triple(TOOL, f"{CATALOG}lineageTo", RESOURCE))
    for index, asset in enumerate((TOOL, RESOURCE)):
        assignment = f"{EXAMPLE}owner-{index}"
        lines += [
            _triple(assignment, RDF_TYPE, f"{CATALOG}StewardshipAssignment"),
            _triple(assignment, f"{CATALOG}assignedTo", asset),
            _triple(assignment, f"{CATALOG}principal", OWNER),
            _triple(assignment, f"{CATALOG}role", f"{CATALOG}owner"),
        ]
        if with_digest:
            lines.append(
                _triple(asset, f"{CONNECTOR_PACK}digest", DIGEST, literal=True)
            )
    return "".join(lines)


@pytest.fixture
def catalog_graph():
    graph = f"medulla-r1063-{uuid.uuid4().hex}"
    admin = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(scopes=None),
    )
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        admin.rdf.icv_configure(SHAPES)
        yield admin
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


def _tool_ids(admin) -> set[str]:
    result = admin.query.uql(f"MATCH () WHERE type = '{CATALOG}ToolAsset'")
    assert result["kind"] == "rows", result
    return {row["id"] for row in result["rows"]}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R106.3")
def test_catalog_batch_is_validated_by_the_guard_and_found_by_query(catalog_graph):
    # spec: MEDULLA-TYPED-PACKS-R106.3
    admin = catalog_graph

    # The batch without the digest is refused and the store is unchanged.
    with pytest.raises(Exception):
        admin.rdf.add_triples(turtle=_batch(with_digest=False))
    assert _tool_ids(admin) == set()

    # The conforming batch is stored and found by served UQL.
    admin.rdf.add_triples(turtle=_batch(with_digest=True))
    assert _tool_ids(admin) == {TOOL}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R106.4")
def test_unresolved_class_unowned_resource_unpaired_edge_and_oversize_batch_refuse(
    catalog_graph,
):
    """Verifies MEDULLA-TYPED-PACKS-R106.4: each refusal has its code and leaves the graph unchanged."""
    admin = catalog_graph
    cases = {
        "UNRESOLVED_REFERENCE": _batch(with_digest=True, declare_class=False),
        "RESOURCE_UNOWNED": _batch(with_digest=True, resource_connector=False),
        "LINEAGE_EDGE_UNPAIRED": _batch(with_digest=True, pair_edge=False),
        "CATALOG_BATCH_TOO_LARGE": _batch(with_digest=True)
        + "".join(
            _triple(f"{EXAMPLE}filler-{i}", f"{CATALOG}note", "x", literal=True)
            for i in range(50_000)
        ),
    }
    for code, batch in cases.items():
        with pytest.raises(Exception, match=code):
            admin.rdf.add_triples(turtle=batch)
        assert _tool_ids(admin) == set(), code
    # A class only the composed schema declares resolves: the schema is the one authority.
    admin.rdf.add_triples(
        turtle=_batch(
            with_digest=True,
            declare_class=False,
            serves="https://knuckles-team.github.io/ontology/core#WorkItem",
        )
    )
    assert _tool_ids(admin) == {TOOL}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R106.5")
def test_asset_carries_digest_version_revision_and_class_counts(catalog_graph):
    """Verifies MEDULLA-TYPED-PACKS-R106.5: the asset keeps its digest and pack version, and
    GraphSchemaClasses reports the asset count per class."""
    import msgpack

    admin = catalog_graph
    graph = admin._client._graph_name
    version = _triple(TOOL, f"{CONNECTOR_PACK}packVersion", "rec-7", literal=True)
    admin.rdf.add_triples(turtle=_batch(with_digest=True) + version)
    raw = admin._send("GetNodeProperties", {"node_id": TOOL}, graph)
    node = msgpack.unpackb(raw, raw=False) if isinstance(raw, (bytes, bytearray)) else raw
    assert node[f"{CONNECTOR_PACK}digest"] == DIGEST
    assert node[f"{CONNECTOR_PACK}packVersion"] == "rec-7"
    page = admin._send(
        "GraphSchemaClasses", {"kind": "class", "cursor": None, "limit": 1000}, graph
    )
    if isinstance(page, (bytes, bytearray)):
        page = msgpack.unpackb(page, raw=False)
    counts = {term["iri"]: term.get("node_count") for term in page["terms"]}
    assert counts[f"{CATALOG}ToolAsset"] == 1
    assert counts[f"{CATALOG}DataResource"] == 1
