"""``GraphSchemaTermView`` carries direct ``superclasses`` (TYPED-PACKS-R812)."""

from __future__ import annotations

import pytest

from medulla.generated.graph_schema import GraphSchemaTermView

pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R812.3")
def test_term_view_carries_superclasses_and_defaults_to_none() -> None:
    term = GraphSchemaTermView.model_validate(
        {
            "iri": "https://example.org/pack#Leaf",
            "kind": "class",
            "local_name": "Leaf",
            "source_id": "admin:tree",
            "superclasses": ["https://example.org/pack#Base"],
        }
    )
    assert term.superclasses == ["https://example.org/pack#Base"]
    bare = GraphSchemaTermView.model_validate(
        {
            "iri": "https://example.org/pack#partOf",
            "kind": "object_property",
            "local_name": "partOf",
            "source_id": "admin:tree",
        }
    )
    assert bare.superclasses == []
