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
CORE_DOCUMENT = "http://knuckles.team/kg/core"
# Module class -> the core class its module names as parent.
MODULE_TO_CORE = {
    f"{KG}BusinessProcess": f"{KG}DomainEntity",
    f"{KG}AgentRole": f"{KG}Role",
    f"{KG}Host": f"{KG}System",
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
