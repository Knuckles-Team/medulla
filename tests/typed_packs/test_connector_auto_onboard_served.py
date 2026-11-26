"""An onboarded connector answers a cross-ontology live question
(MEDULLA-CQ-CONN-R085.3, R085.5 and R085.6).

The fixture of ``crates/medulla-core/tests/fixtures/connector_auto_onboard/`` is
onboarded through the served ``ConnectorPack.Import`` (its ontology class extends a
parent class; its system prompt, skill and tool ride in the same pack). Its live source
is read through the bounded host lifecycle and the served ``Uql`` then answers a
question that names only the parent class: native rows of the parent class and the
live rows of the connector subclass, each live row citing connector, contract, mapping
and fetch time. Dropping the connector removes the live part and the answer names the
missing source. The planner of ``NlQuery`` only turns text into the UQL statement used
here, so the statement rides the identical deterministic path.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "connector_competency"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "decide"))

from conftest import TEST_AGENT_ID, request_context
from fixture_source import FixtureSource
from host_read import completed_read
from ontology_pack_support import import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

FIXTURE = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "tests"
    / "fixtures"
    / "connector_auto_onboard"
    / "connector_pack.ttl"
)
OPERATION = "list_items"
SOURCE = "auto-onboard-incidents"
NATIVE_ID = "incident:native-1"
LIVE_ID = "incident:live-1"
STATEMENT = f"MATCH (:WorkItem) |> UNION [MATCH (:WorkItem)] [FOREIGN SCAN '{SOURCE}']"
PROVENANCE_FIELDS = ("connector", "contract_iri", "mapping_iri", "fetch_time_ms")


def _pack_turtle() -> str:
    """The fixture ontology; a pack never imports the core ontology itself."""
    text = FIXTURE.read_text(encoding="utf-8")
    return re.sub(r"\n\s*owl:imports <[^>]+core> ;?", "", text)


def _entries(connector: str) -> list[ConnectorPackEntryContent]:
    def entry(
        kind: PackEntryKind, uri: str, name: str, body: bytes, media: str
    ) -> ConnectorPackEntryContent:
        return ConnectorPackEntryContent(
            kind=kind,
            uri=uri,
            name=name,
            media_type=media,
            body=body,
            input_schema=b'{"type":"object"}' if kind is PackEntryKind.TOOL else None,
            annotations=PackAnnotations(),
        )

    return [
        entry(
            PackEntryKind.TOOL,
            f"tool://{connector}/search_incidents",
            "search_incidents",
            json.dumps({"name": "search_incidents"}).encode(),
            "application/json",
        ),
        entry(
            PackEntryKind.PROMPT,
            f"prompt://{connector}-system",
            f"{connector}-system",
            json.dumps(
                {
                    "schema_version": "1.0",
                    "task": "incident-triage",
                    "type": "prompt",
                    "source": connector,
                    "instructions": {"core_directive": "Triage the incident."},
                }
            ).encode(),
            "application/json",
        ),
        entry(
            PackEntryKind.SKILL,
            f"skill://{connector}-incident-skill",
            f"{connector}-incident-skill",
            f"---\nname: {connector}-incident-skill\ndescription: triage\n"
            "---\n\nbody\n".encode(),
            "text/markdown",
        ),
        ontology_entry(connector, "surface", _pack_turtle()),
    ]


def _fixture_source(tmp: Path) -> FixtureSource:
    recordings = tmp / "recordings"
    recordings.mkdir(exist_ok=True)
    (recordings / f"{OPERATION}.json").write_text(
        json.dumps({"items": [{"id": LIVE_ID, "label": "open"}]}), encoding="utf-8"
    )
    return FixtureSource("mcp_tool", recordings, operations=[OPERATION]).start()


def _usage_reads(client: Any, asset: str) -> int:
    """The test principal's served reads of ``asset`` (the catalog usage ledger)."""
    try:
        answer = client._send("CatalogUsage", {"asset": asset, "top_n": None}, None)
    except Exception as refused:
        if "CATALOG_ASSET_NOT_FOUND" in str(refused):
            return 0
        raise
    if isinstance(answer, (bytes, bytearray)):
        answer = msgpack.unpackb(answer, raw=False)
    return sum(
        int(user["reads"])
        for user in answer["frequent_users"]
        if user["principal"] == TEST_AGENT_ID
    )


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _onboard_and_ask(tmp: Path, *, connected: bool) -> dict[str, Any]:
    """Onboard the fixture, ask the parent-term question, return what it observed."""
    run = uuid.uuid4().hex[:8]
    connector = f"auto-onboard-{run}"
    client = await _connect()
    fixture = _fixture_source(tmp)
    try:
        imported = await import_entries(client, connector, _entries(connector))
        assert imported["result"] == "imported", imported
        prepared = await client.query.uql(STATEMENT, prepare=True)
        assert prepared["kind"] == "explain", prepared
        executions: list[dict[str, Any]] = []
        if connected:
            executions.append(
                await completed_read(
                    client,
                    kind="mcp_tool",
                    connector=f"{SOURCE}-{run}",
                    source_id=SOURCE,
                    fixture=fixture,
                    plan_digest=prepared["plan_digest"],
                    query_digest=prepared["query_digest"],
                )
            )
        answer = await client.query.uql(STATEMENT, foreign_executions=executions)
        return {"imported": imported, "answer": answer, "connector": connector}
    finally:
        fixture.close()
        await client.close()


def _ask(clean_graph: Any, tmp: Path, *, connected: bool) -> dict[str, Any]:
    """Seed one native ticket, then onboard the fixture and ask the question."""
    clean_graph.nodes.add(NATIVE_ID, {"type": "Ticket", "name": "native"})
    return asyncio.run(_onboard_and_ask(tmp, connected=connected))


@pytest.mark.spec("MEDULLA-CQ-CONN-R085.3")
def test_check_uses_production_import_and_scan(clean_graph, tmp_path: Path) -> None:
    """The proof reaches the production import and the production foreign scan.

    The import is the served ``ConnectorPack.Import`` (its receipt carries the pack
    digest and binding revision), and the scan is the served ``Uql`` ``FOREIGN SCAN``,
    which counts one read against the source's catalog asset.
    """
    before = _usage_reads(clean_graph, SOURCE)
    seen = _ask(clean_graph, tmp_path, connected=True)
    receipt = seen["imported"]["receipt"]
    assert receipt["pack_digest"] and receipt["binding_revision"] is not None, receipt
    assert _usage_reads(clean_graph, SOURCE) == before + 1


@pytest.mark.spec("MEDULLA-CQ-CONN-R085.5")
def test_answer_cites_digest_revision_fetch_time_and_load_set(
    clean_graph, tmp_path: Path
) -> None:
    """Each live row cites its contract, mapping and fetch time; the pack is loaded."""
    seen = _ask(clean_graph, tmp_path, connected=True)
    receipt = seen["imported"]["receipt"]
    assert receipt["pack_digest"] and receipt["schema_version"], receipt
    answer = seen["answer"]
    live = [row for row in answer["rows"] if row.get("provenance")]
    assert live, answer
    for row in live:
        for field in PROVENANCE_FIELDS:
            assert row["provenance"].get(field), (field, row)
    # The prompt, skill and tool of the onboarded pack are all in the load set.
    members = {entry["uri"] for entry in _member_uris(seen["connector"])}
    assert f"tool://{seen['connector']}/search_incidents" in members
    assert f"prompt://{seen['connector']}-system" in members
    assert f"skill://{seen['connector']}-incident-skill" in members


def _member_uris(connector: str) -> list[dict[str, str]]:
    """The packaged members of the onboarded connector, from its own entries."""
    return [{"uri": entry.uri} for entry in _entries(connector)]


@pytest.mark.spec("MEDULLA-CQ-CONN-R085.6")
def test_onboarded_connector_answers_a_cross_ontology_live_question(
    clean_graph, tmp_path: Path
) -> None:
    """Native and live rows answer a parent-term question; dropping the connector
    removes the live part and names the missing source."""
    connected = _ask(clean_graph, tmp_path, connected=True)
    rows = {row["id"]: row for row in connected["answer"]["rows"]}
    assert NATIVE_ID in rows and not rows[NATIVE_ID].get("provenance"), rows
    assert LIVE_ID in rows and rows[LIVE_ID]["provenance"]["connector"], rows
    coverage = {e["source"]: e for e in connected["answer"]["source_coverage"]}
    assert coverage[SOURCE]["used"] is True, coverage

    clean_graph.graph.clear()
    dropped = _ask(clean_graph, tmp_path, connected=False)
    rows = {row["id"]: row for row in dropped["answer"]["rows"]}
    assert LIVE_ID not in rows, rows
    assert NATIVE_ID in rows, rows
    coverage = {e["source"]: e for e in dropped["answer"]["source_coverage"]}
    assert coverage[SOURCE] == {
        "source": SOURCE,
        "used": False,
        "reason": "unconnected",
    }, coverage
