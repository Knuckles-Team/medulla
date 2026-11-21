"""MEDULLA-TYPED-PACKS-R700.3 and R700.4 served acceptance: skill graph bundles.

A pack ``Skill`` entry that declares ``skill_type: graph`` names a graph whose
``skill_file`` entries (``SKILL.md``, ``index.json``, ``sources.json`` and the
``reference/`` files) are validated inside the served ``ConnectorPack.Import``
before anything commits. A conformant bundle imports atomically and its skill
renders on demand in the served fleet catalog listing. A bundle with a wrong
digest, no ``index.json`` or a manifest path with no entry is refused with the
code that names the failing path, and the refused import leaves the durable pack
head and its conformance evidence unchanged.
"""

from __future__ import annotations

import hashlib
import json
import sys
import uuid
from pathlib import Path

import pytest
from conftest import TEST_TENANT

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries
from test_connector_pack_planted import _context, _pack_op

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.fleet_catalog import FleetCatalogClient
from medulla.generated.connector_pack import (
    ConformanceEvidence,
    PackAnnotations,
    PackEntryKind,
    PackHeadRef,
    PackRef,
)
from medulla.generated.fleet_catalog import (
    DiscoveryCounts,
    DiscoveryOutcomeReachable,
    DiscoveryScopeTenantLocal,
    FleetDiscoveryRecordRequest,
)

ABSENT = "CONFORMANCE_EVIDENCE_ABSENT"


def _sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def _file(graph: str, path: str, text: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.SKILL_FILE,
        uri=f"skill-file://{graph}/{path}",
        name=f"{graph}/{path}",
        media_type="application/json" if path.endswith(".json") else "text/markdown",
        body=text.encode(),
        annotations=PackAnnotations(),
        references=(PackRef(kind=PackEntryKind.SKILL, uri=f"skill://{graph}/SKILL.md"),),
    )


def _graph_entries(
    graph: str,
    *,
    bad_hash: bool = False,
    with_index: bool = True,
    ghost_path: bool = False,
) -> list[ConnectorPackEntryContent]:
    skill_md = f"---\nname: {graph}\ndescription: A graph skill.\nskill_type: graph\n---\n# {graph}\n"
    reference = "reference one\n"
    index = json.dumps({"sections": [{"path": "reference/one.md"}]})
    declared = {
        "SKILL.md": _sha(skill_md),
        "reference/one.md": _sha("other text" if bad_hash else reference),
        "index.json": _sha(index),
    }
    if ghost_path:
        declared["reference/ghost.md"] = _sha("ghost")
    sources = json.dumps(
        {"files": [{"path": p, "sha256": f"sha256:{d}"} for p, d in declared.items()]}
    )
    skill = ConnectorPackEntryContent(
        kind=PackEntryKind.SKILL,
        uri=f"skill://{graph}/SKILL.md",
        name=graph,
        media_type="text/markdown",
        body=skill_md.encode(),
        annotations=PackAnnotations(),
    )
    entries = [
        skill,
        _file(graph, "SKILL.md", skill_md),
        _file(graph, "reference/one.md", reference),
        _file(graph, "sources.json", sources),
    ]
    if with_index:
        entries.append(_file(graph, "index.json", index))
    return entries


async def _evidence(client: MedullaClient, connector: str) -> ConformanceEvidence:
    raw = await _pack_op(
        client,
        "conformance_evidence",
        {"tenant_id": TEST_TENANT, "connector": connector},
        f"graph-evidence:{connector}:{uuid.uuid4().hex}",
    )
    return ConformanceEvidence.model_validate(raw)


def _details(result: dict) -> list[str]:
    assert result["result"] == "rejected", result
    return [violation["detail"] for violation in result["violations"]]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R700.3")
@pytest.mark.asyncio
async def test_bundle_validated_in_import_and_candidate_renders_on_demand() -> None:
    client = await connect()
    try:
        connector = f"graph-{uuid.uuid4().hex[:8]}"
        graph = "billing"

        # A bundle whose digest is wrong is validated in the import and refused.
        bad = await import_entries(
            client, connector, _graph_entries(graph, bad_hash=True)
        )
        assert any(
            d.startswith("SKILL_GRAPH_HASH_MISMATCH: reference/one.md")
            for d in _details(bad)
        ), bad

        # A conformant bundle commits through the one atomic import.
        imported = await import_entries(client, connector, _graph_entries(graph))
        assert imported["result"] == "imported", imported
        evidence = await _evidence(client, connector)
        assert evidence.pack_digest == imported["receipt"]["pack_digest"]

        # The skill renders on demand in the served catalog listing.
        catalog = FleetCatalogClient(client)
        await catalog.record_discovery(
            FleetDiscoveryRecordRequest(
                server_name=f"{connector}-server",
                scope=DiscoveryScopeTenantLocal(authority="tenant_local"),
                connector=connector,
                outcome=DiscoveryOutcomeReachable(probe="reachable"),
                counts=DiscoveryCounts(prompts=0, resources=0, skills=1, tools=0),
            )
        )
        rows = await catalog.list_all("skills")
        mine = [
            row.row
            for row in rows
            if row.row.component.connector == connector
            and row.row.component.name == graph
        ]
        assert len(mine) == 1, rows
        assert mine[0].package is not None, mine[0].package_refusal
        assert len(mine[0].package.digest) == 64
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R700.4")
@pytest.mark.asyncio
async def test_hash_index_and_entry_refusals_leave_state_unchanged() -> None:
    client = await connect()
    try:
        connector = f"graph-refuse-{uuid.uuid4().hex[:8]}"
        graph = "payments"
        cases = {
            "SKILL_GRAPH_HASH_MISMATCH: reference/one.md": {"bad_hash": True},
            "SKILL_GRAPH_INDEX_MISSING: index.json": {"with_index": False},
            "SKILL_GRAPH_ENTRY_MISSING: reference/ghost.md": {"ghost_path": True},
        }

        # A refused first import commits nothing: no evidence exists afterwards.
        for expected, options in cases.items():
            result = await import_entries(
                client, connector, _graph_entries(graph, **options)
            )
            details = _details(result)
            assert any(d.startswith(expected) for d in details), (expected, details)
            with pytest.raises(EngineResponseError, match=ABSENT):
                await _evidence(client, connector)

        # A refused reimport leaves the committed head and evidence as they were.
        first = await import_entries(client, connector, _graph_entries(graph))
        assert first["result"] == "imported", first
        before = await _evidence(client, connector)
        head = first["receipt"]
        expected_head = PackHeadRef(
            binding_revision=head["binding_revision"], pack_digest=head["pack_digest"]
        )
        for expected, options in cases.items():
            result = await import_entries(
                client,
                connector,
                _graph_entries(graph, **options),
                expected_head=expected_head,
            )
            assert result["result"] == "rejected", (expected, result)
        after = await _evidence(client, connector)
        assert after == before
        assert after.pack_digest == head["pack_digest"]
    finally:
        await client.close()
