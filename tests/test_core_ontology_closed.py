"""The core ontology is one closed class hierarchy rooted at the BFO entity class.

Every class a packaged core module declares reaches ``bfo:0000001`` through
``rdfs:subClassOf`` edges whose parents are themselves core classes, so a pack
class that extends any core class rolls up to the one root. The core names the
typed state-machine refusal as a class that a pack's per-machine refusal class
extends (MEDULLA-TYPED-PACKS-R105.10.1, MEDULLA-ORCH-VOCAB-R811.10).
"""

from __future__ import annotations

from functools import cache

import pyoxigraph
import pytest

from medulla import ontology

pytestmark = [pytest.mark.no_engine]

CORE = "https://knuckles-team.github.io/ontology/core#"
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
def _triples() -> tuple[tuple[str, str, object], ...]:
    """Named-subject triples of every packaged core module (shapes excluded)."""
    found: list[tuple[str, str, object]] = []
    for name, body in ontology.files():
        if name.endswith(ontology.SHAPES_SUFFIX):
            continue
        for triple in pyoxigraph.parse(body, format=pyoxigraph.RdfFormat.TURTLE):
            if isinstance(triple.subject, pyoxigraph.NamedNode):
                found.append(
                    (triple.subject.value, triple.predicate.value, triple.object)
                )
    return tuple(found)


def _classes() -> set[str]:
    return {
        s
        for s, p, o in _triples()
        if p == RDF_TYPE
        and isinstance(o, pyoxigraph.NamedNode)
        and o.value in CLASS_TYPES
    }


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
