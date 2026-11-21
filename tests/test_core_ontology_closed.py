"""The core ontology is one closed class hierarchy rooted at the BFO entity class.

Every class a packaged core module declares reaches ``bfo:0000001`` through
``rdfs:subClassOf`` edges whose parents are themselves core classes, so a pack
class that extends any core class rolls up to the one root. The core names the
typed state-machine refusal as a class that a pack's per-machine refusal class
extends (MEDULLA-TYPED-PACKS-R105.10.1, MEDULLA-ORCH-VOCAB-R811.10). Each class
is declared by one module, and no local name names two classes across the core
namespaces, so a name resolves to one class (MEDULLA-TYPED-PACKS-R105.10.2,
MEDULLA-TYPED-PACKS-R105.10.3).
"""

from __future__ import annotations

from collections import defaultdict
from functools import cache

import pyoxigraph
import pytest

from medulla import ontology

pytestmark = [pytest.mark.no_engine]

ONE_BASE = "https://knuckles-team.github.io/ontology/"
CORE = ONE_BASE + "core#"
ROOT = "http://purl.obolibrary.org/obo/BFO_0000001"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
ALT_LABEL = "http://www.w3.org/2004/02/skos/core#altLabel"
CLASS_TYPES = frozenset(
    {
        "http://www.w3.org/2002/07/owl#Class",
        "http://www.w3.org/2000/01/rdf-schema#Class",
    }
)


@cache
def _module_triples() -> tuple[tuple[str, tuple[tuple[str, str, object], ...]], ...]:
    """Named-subject triples of each packaged core module (shapes excluded)."""
    modules: list[tuple[str, tuple[tuple[str, str, object], ...]]] = []
    for name, body in ontology.files():
        if name.endswith(ontology.SHAPES_SUFFIX):
            continue
        found = tuple(
            (triple.subject.value, triple.predicate.value, triple.object)
            for triple in pyoxigraph.parse(body, format=pyoxigraph.RdfFormat.TURTLE)
            if isinstance(triple.subject, pyoxigraph.NamedNode)
        )
        modules.append((name, found))
    return tuple(modules)


@cache
def _triples() -> tuple[tuple[str, str, object], ...]:
    """Named-subject triples of every packaged core module (shapes excluded)."""
    return tuple(t for _, found in _module_triples() for t in found)


def _declares_class(triple: tuple[str, str, object]) -> bool:
    _, p, o = triple
    return (
        p == RDF_TYPE and isinstance(o, pyoxigraph.NamedNode) and o.value in CLASS_TYPES
    )


def _classes() -> set[str]:
    return {t[0] for t in _triples() if _declares_class(t)}


def _local_name(iri: str) -> str:
    return iri.rsplit("#", 1)[-1].rsplit("/", 1)[-1]


def _core_parents(classes: set[str]) -> dict[str, set[str]]:
    parents: dict[str, set[str]] = {}
    for s, p, o in _triples():
        if p == SUBCLASS and isinstance(o, pyoxigraph.NamedNode) and o.value in classes:
            parents.setdefault(s, set()).add(o.value)
    return parents


def _reaches_root(cls: str, parents: dict[str, set[str]]) -> bool:
    seen: set[str] = set()
    frontier = [cls]
    while frontier:
        current = frontier.pop()
        if current == ROOT:
            return True
        if current not in seen:
            seen.add(current)
            frontier.extend(parents.get(current, ()))
    return False


# spec: MEDULLA-TYPED-PACKS-R105.10.1
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R105.10.1")
def test_every_core_class_reaches_the_root() -> None:
    classes = _classes()
    assert ROOT in classes
    parents = _core_parents(classes)
    detached = sorted(c for c in classes if not _reaches_root(c, parents))
    assert not detached, (
        f"{len(detached)} core class(es) reach no bfo:0000001 through core classes: "
        f"{detached}"
    )


# spec: MEDULLA-ORCH-VOCAB-R811.10
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R811.10")
def test_transition_refused_is_a_core_refusal_class() -> None:
    refused = CORE + "TransitionRefused"
    assert refused in _classes()
    facts = [(p, o) for s, p, o in _triples() if s == refused]
    assert (SUBCLASS, pyoxigraph.NamedNode(CORE + "RefusalCode")) in facts
    assert any(p == LABEL for p, _ in facts)
    aliases = {o.value for p, o in facts if p == ALT_LABEL}
    assert {"Transition Refusal", "Lifecycle Refusal"} <= aliases
    guard_aliases = {
        o.value
        for s, p, o in _triples()
        if s == CORE + "TransitionGuard" and p == ALT_LABEL
    }
    assert "Guard" in guard_aliases
    assert CORE + "Guard" not in _classes(), (
        "a guard is a :TransitionGuard; no second class"
    )


# spec: MEDULLA-TYPED-PACKS-R105.10.2
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R105.10.2")
def test_every_core_class_has_one_home() -> None:
    homes: dict[str, set[str]] = defaultdict(set)
    for module, found in _module_triples():
        for triple in found:
            if _declares_class(triple):
                homes[triple[0]].add(module)
    repeated = {c: sorted(m) for c, m in homes.items() if len(m) > 1}
    assert not repeated, f"classes declared by more than one module: {repeated}"


# spec: MEDULLA-TYPED-PACKS-R105.10.3
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R105.10.3")
def test_no_local_name_repeats_across_core_namespaces() -> None:
    by_name: dict[str, set[str]] = defaultdict(set)
    for cls in _classes():
        if cls.startswith(ONE_BASE):
            by_name[_local_name(cls)].add(cls)
    repeated = {n: sorted(c) for n, c in by_name.items() if len(c) > 1}
    assert not repeated, f"local names naming more than one core class: {repeated}"
