"""Served core corpus acceptance (MEDULLA-TYPED-PACKS-R071.2).

Shipped modules extend core classes through ``rdfs:subClassOf`` and never
import the core document; the served ``OntologyInspect`` reads the same
composed corpus the engine validates against.
"""

from __future__ import annotations

import asyncio

import pytest
from ontology_pack_support import connect, inspect_committed

KG = "http://knuckles.team/kg#"
INFRA = "http://knuckles.team/kg/infrastructure#"
BFO_ENTITY = "http://purl.obolibrary.org/obo/BFO_0000001"
CORE_DOCUMENT = "http://knuckles.team/kg/core"
# Module class -> the core class its module names as parent. The archimate
# module mixes continuants, processes and information entities, so it roots
# under bfo:Entity: no narrower core parent is consistent with BFO disjointness.
MODULE_TO_CORE = {
    f"{KG}BusinessProcess": BFO_ENTITY,
    f"{KG}AgentRole": f"{KG}Role",
    f"{INFRA}Host": f"{KG}System",
    f"{KG}TRMStandard": f"{KG}Policy",
    f"{KG}Hook": f"{KG}Framework",
}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.2")
def test_modules_extend_core_through_subclass() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            view = await inspect_committed(client)
            classes = {cls.iri: cls for cls in view.classes}
            for module_class, core_class in MODULE_TO_CORE.items():
                assert core_class in classes, core_class
                assert core_class in classes[module_class].parents, module_class
            # The core document is a composed source, so module classes still
            # resolve, but no module header imports it.
            assert CORE_DOCUMENT in view.ontologies
        finally:
            await client.close()

    asyncio.run(exercise())
