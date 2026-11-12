"""Served food-composition pack import (MEDULLA-MEDIA-VOCAB-R655).

A food-data pack ontology carries ``:NutrientAmount`` individuals. Each names its
source nutrient identifier; the nutrient crosswalk maps it to a chemical-entity
term. An identifier without a crosswalk entry refuses the whole import with
``NUTRIENT_ID_UNMAPPED``.
"""

from __future__ import annotations

import asyncio
import sys
import time
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

from ontology_pack_support import (
    connect,
    import_entries,
    ontology_entry,
    violation_codes,
)

PREFIXES = (
    "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n"
    "@prefix obo: <http://purl.obolibrary.org/obo/> .\n"
    "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
)


def _food(connector: str, nutrient_id: str, chebi: str) -> str:
    return (
        f"{PREFIXES}"
        f':food_{connector} a :FoodTerm ; rdfs:label "Oats" .\n'
        f':record_{connector} a :FoodCompositionRecord ; :fdcId "173904" ;'
        f" :describesFood :food_{connector} ;"
        f" :hasNutrientAmount :amount_{connector} .\n"
        f':amount_{connector} a :NutrientAmount ; :nutrientId "{nutrient_id}" ;'
        f" :amountOfNutrient obo:{chebi} .\n"
    )


@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R655")
def test_food_pack_import_answers_composition_query() -> None:
    async def exercise() -> None:
        client = await connect()
        try:
            connector = f"food-{uuid.uuid4().hex[:8]}"
            refused = await import_entries(
                client,
                connector,
                [
                    ontology_entry(
                        connector, "foods", _food(connector, "9999", "CHEBI_36080")
                    )
                ],
            )
            assert "NUTRIENT_ID_UNMAPPED" in violation_codes(refused)

            connector = f"food-{uuid.uuid4().hex[:8]}"
            accepted = await import_entries(
                client,
                connector,
                [
                    ontology_entry(
                        connector, "foods", _food(connector, "1003", "CHEBI_36080")
                    )
                ],
            )
            assert accepted["result"] != "rejected", accepted
            # The pack graph is projected after the import commits; its rows
            # join the default graph once the projection is visible.
            deadline = time.monotonic() + 180
            while True:
                rows = await client.rdf.sparql(
                    "PREFIX : <https://knuckles-team.github.io/ontology/core#> "
                    "SELECT ?id WHERE { ?amount a :NutrientAmount ; :nutrientId ?id }"
                )
                if any(row.get("id") == "1003" for row in rows):
                    break
                assert time.monotonic() < deadline, rows
                await asyncio.sleep(0.5)
        finally:
            await client.close()

    asyncio.run(exercise())
