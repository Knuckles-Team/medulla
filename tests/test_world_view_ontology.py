"""The world-view spatial vocabulary extends the core ontology (MEDULLA-WORLD-VIEW-R001)."""

from __future__ import annotations

import pyoxigraph
import pytest

from medulla import ontology

pytestmark = [pytest.mark.no_engine]

ONE_BASE = "https://knuckles-team.github.io/ontology/"
CORE = ONE_BASE + "core#"
MODULE = ONE_BASE + "worldview"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
CLASS = "http://www.w3.org/2002/07/owl#Class"
OBJECT_PROPERTY = "http://www.w3.org/2002/07/owl#ObjectProperty"
DATATYPE_PROPERTY = "http://www.w3.org/2002/07/owl#DatatypeProperty"
ONTOLOGY = "http://www.w3.org/2002/07/owl#Ontology"
SPATIAL_CLASSES = ("District", "Building", "Room", "Street")
IDENTITY_CLASSES = ("Agent", "Memory", "Workflow", "Store", "Identity")


def _triples(file_name: str) -> list[tuple[str, str, object]]:
    body = ontology.read(file_name)
    return [
        (t.subject.value, t.predicate.value, t.object)
        for t in pyoxigraph.parse(body, format=pyoxigraph.RdfFormat.TURTLE)
        if isinstance(t.subject, pyoxigraph.NamedNode)
    ]


def _declared(triples: list[tuple[str, str, object]], kind: str) -> set[str]:
    return {
        s
        for s, p, o in triples
        if p == RDF_TYPE and isinstance(o, pyoxigraph.NamedNode) and o.value == kind
    }


def _world_view_file() -> str:
    return ontology.module_files()[MODULE]


# spec: MEDULLA-WORLD-VIEW-R001
@pytest.mark.spec("MEDULLA-WORLD-VIEW-R001")
def test_spatial_classes_extend_the_core_ontology() -> None:
    file_name = _world_view_file()
    assert file_name in ontology.names(), "the world-view module is packaged"
    triples = _triples(file_name)
    assert MODULE in _declared(triples, ONTOLOGY)

    classes = _declared(triples, CLASS)
    for name in SPATIAL_CLASSES:
        iri = CORE + name
        assert iri in classes, f"{name} is declared under the shared ontology base"
        parents = {
            o.value
            for s, p, o in triples
            if s == iri and p == SUBCLASS and isinstance(o, pyoxigraph.NamedNode)
        }
        assert parents == {CORE + "Place"}, f"{name} extends the core Place"

    properties = _declared(triples, OBJECT_PROPERTY) | _declared(triples, DATATYPE_PROPERTY)
    assert CORE + "hasInterior" in properties
    assert CORE + "position" in properties
    core_triples = _triples("core-foundation-v1.ttl")
    assert CORE + "locatedIn" in _declared(core_triples, OBJECT_PROPERTY), (
        "locatedIn is the core relation, reused and not redeclared"
    )
    assert CORE + "locatedIn" not in properties
    assert CORE + "Place" in _declared(core_triples, CLASS)

    local_names = {iri.rsplit("#", 1)[-1] for iri in classes}
    assert not local_names.intersection(IDENTITY_CLASSES), (
        "no parallel identity, memory, orchestration or store class is introduced"
    )
    for iri in classes:
        assert iri.startswith(CORE), "every class is under the shared ontology base"
