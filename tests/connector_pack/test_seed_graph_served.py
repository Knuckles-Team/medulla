"""Served seed graph import (MEDULLA-TYPED-PACKS-R104.6).

A connector pack carries its seed as up to three ``seed_part`` entries. The import
receipt names each part's outcome, a repeat of equal parts is ``unchanged``, a
restart changes nothing (the prior state is the pack head's own seed members), and
changing one part replaces only that part. The projected pack graph answers a SPARQL
question about the new objects, each seed subject linked to the seed revision node
the receipt names.
"""

from __future__ import annotations

import asyncio
import hashlib
import struct
import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))

from ontology_pack_support import import_entries
from test_pack_contract_served import _engine, _head

from conftest import TEST_TENANT, request_context  # isort: skip
from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

GRAPH_DOMAIN = b"eg/connector-pack-projection-graph/v1"


def _pack_graph(tenant: str, connector: str) -> str:
    """The physical pack graph name (``Digest256::framed`` over tenant and connector)."""
    hasher = hashlib.sha256()
    hasher.update(b"eg/framed-sha256/v1\0")
    hasher.update(struct.pack(">I", len(GRAPH_DOMAIN)))
    hasher.update(GRAPH_DOMAIN)
    fields = [tenant.encode(), connector.encode()]
    hasher.update(struct.pack(">I", len(fields)))
    for field in fields:
        hasher.update(struct.pack(">Q", len(field)))
        hasher.update(field)
    return f"pack__{hasher.hexdigest()}"


def _seed(key: str, turtle: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.SEED_PART,
        uri=f"seed://{key}",
        name=key,
        media_type="text/turtle",
        body=turtle.encode(),
        annotations=PackAnnotations(),
    )


def _parts(catalog_subject: str) -> list[ConnectorPackEntryContent]:
    return [
        _seed("identity", "<urn:seed:instance> a <urn:seed:Identity> .\n"),
        _seed("catalog", f"<{catalog_subject}> a <urn:seed:CatalogEntry> .\n"),
        _seed(
            "references",
            "<urn:seed:item> <urn:seed:references> <urn:source:item> .\n",
        ),
    ]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R104.6")
def test_seed_idempotent_and_partial_reimport(tmp_path: Path) -> None:
    connector = f"seed-served-{uuid.uuid4().hex[:8]}"
    persist = tmp_path / "persist"
    persist.mkdir()
    state: dict[str, object] = {}

    async def first_run(socket_path: str) -> None:
        admin = await MedullaClient.connect(
            socket_path=socket_path, verified_context=request_context()
        )
        try:
            imported = await import_entries(admin, connector, _parts("urn:seed:old"))
            assert imported["result"] == "imported", imported
            outcomes = imported["receipt"]["seed"]["outcomes"]
            assert set(outcomes.values()) == {"inserted"}, outcomes
            state["head"] = _head(imported["receipt"])
            state["first_revision"] = imported["receipt"]["seed"]["graph_revision"]
            # Repeating the digest is a no-op: the head already holds this pack.
            repeated = await import_entries(
                admin, connector, _parts("urn:seed:old"), expected_head=state["head"]
            )
            assert repeated["result"] == "unchanged", repeated
        finally:
            await admin.close()

    async def second_run(socket_path: str) -> None:
        admin = await MedullaClient.connect(
            socket_path=socket_path, verified_context=request_context()
        )
        try:
            # After a restart the repeat is still unchanged.
            repeated = await import_entries(
                admin, connector, _parts("urn:seed:old"), expected_head=state["head"]
            )
            assert repeated["result"] == "unchanged", repeated
            # Changing the catalog part replaces that part alone.
            changed = await import_entries(
                admin, connector, _parts("urn:seed:new"), expected_head=state["head"]
            )
            assert changed["result"] == "imported", changed
            seed = changed["receipt"]["seed"]
            assert seed["outcomes"] == {
                "identity": "unchanged",
                "catalog": "replaced",
                "references": "unchanged",
            }, seed
            assert seed["graph_revision"] != state["first_revision"]
            revision = seed["graph_revision"]
        finally:
            await admin.close()

        graph = _pack_graph(TEST_TENANT, connector)
        deadline = asyncio.get_running_loop().time() + 120
        rows: list[dict[str, str | None]] = []
        while asyncio.get_running_loop().time() < deadline:
            client = await MedullaClient.connect(
                socket_path=socket_path,
                verified_context=request_context(),
                graph_name=graph,
            )
            try:
                rows = await client.sparql(
                    "SELECT ?subject WHERE { ?subject "
                    "<https://knuckles-team.github.io/ontology/connector-seed#fromSeed> "
                    f"<{revision}> }} ORDER BY ?subject"
                )
            except Exception:  # the projection has not landed yet
                rows = []
            finally:
                await client.close()
            if rows:
                break
            await asyncio.sleep(1)
        assert [row["subject"] for row in rows] == [
            "urn:seed:instance",
            "urn:seed:item",
            "urn:seed:new",
        ], rows

    with _engine(tmp_path, persist, TEST_TENANT, bootstrap=True) as socket_path:
        asyncio.run(first_run(socket_path))
    with _engine(tmp_path, persist, TEST_TENANT, bootstrap=False) as socket_path:
        asyncio.run(second_run(socket_path))
