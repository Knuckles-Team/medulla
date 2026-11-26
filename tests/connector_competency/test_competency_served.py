"""Table-driven served acceptance of the connector questions
(MEDULLA-CQ-CONN-R067.9) and its abstention variants (MEDULLA-CQ-CONN-R067.10).

One parametrised test per case of ``cases/<connector>.json``. Each case imports the
connector pack of every source it names, approves each mapping through served
``ConnectorPackOp::ApproveSource``, starts one ``FixtureSource`` per source and
reads it through the bounded host lifecycle (``host_read.completed_read``), then
runs the case ``uql`` through served ``Uql`` with ``foreign_executions`` mapping
each source name to the ``execution`` it returned. It asserts the columns,
``min_rows`` and the provenance of every row, that no source row is stored (the
store audit of ``audit.stored_nodes``), that every listed term resolves in the
module schema source through served ``Sparql``, and that the same statement read
under a second tenant returns no row. A cross-source case first writes the
approved ``owl:sameAs`` links through served ``SourceIngest``.
"""

from __future__ import annotations

import asyncio
import copy
import json
import os
import re
import shutil
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from _sparql_solutions import solution_values
from audit import stored_nodes
from cases import RECORDINGS_DIR, Case, load_cases
from conftest import TEST_AGENT_ID, request_context, tenant_request_context
from fixture_source import FixtureSource
from host_read import completed_read
from test_contract_kinds_served import (
    HELD_REFUSAL,
    OPERATION,
    UNAVAILABLE_REFUSAL,
    _host_fetch,
    _named_refusal,
)

from medulla.client import MedullaClient

CASES = load_cases()
OTHER_TENANT = "tenant:competency-other"
HELD_CODES = ("SOURCE_HELD", HELD_REFUSAL)
ABSENT_MARKERS = ("SOURCE_REGISTRATION_NOT_FOUND", "unconnected", "Unconnected")
VARIANTS = ("absent", "held", "stopped")


def _row_id(case: Case) -> str:
    """The per-connector row a case binds: ``.1`` single, ``.2`` cross."""
    return f"{case.rollup}.{1 if case.kind == 'single' else 2}"


def _params(variants: tuple[str, ...] | None = None) -> list[Any]:
    out = []
    for case in CASES:
        marks = [pytest.mark.spec(_row_id(case))]
        if variants is None:
            out.append(pytest.param(case, id=case.id, marks=marks))
            continue
        for variant in variants:
            out.append(
                pytest.param(case, variant, id=f"{case.id}-{variant}", marks=marks)
            )
    return out


def _stage(tmp: Path, case: Case, source: str) -> Path:
    """A directory holding the recording of ``source`` as the contract operation."""
    recorded = [r for r in case.recordings if r.split("/", 1)[0] == source]
    target = tmp / source
    target.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(RECORDINGS_DIR / recorded[0], target / f"{OPERATION}.json")
    return target


def _fixture(tmp: Path, case: Case, source: str) -> FixtureSource:
    return FixtureSource(
        case.source_kinds[source], _stage(tmp, case, source), operations=[OPERATION]
    ).start()


async def _connect(context: dict[str, object] | None = None) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context or request_context(),
    )


async def _prepare(client: MedullaClient, case: Case) -> tuple[str, str]:
    prepared = await client.query.uql(case.uql, prepare=True)
    assert prepared["kind"] == "explain", prepared
    return prepared["plan_digest"], prepared["query_digest"]


async def _read(
    client: MedullaClient,
    case: Case,
    tmp: Path,
    source: str,
    run: str,
    digests: tuple[str, str],
    approve: bool = True,
) -> tuple[dict[str, Any], FixtureSource]:
    fixture = _fixture(tmp, case, source)
    execution = await completed_read(
        client,
        kind=case.source_kinds[source],
        connector=f"{source}-{run}",
        source_id=source,
        fixture=fixture,
        plan_digest=digests[0],
        query_digest=digests[1],
        approve=approve,
    )
    return execution, fixture


def _items(case: Case, source: str) -> list[dict[str, Any]]:
    """The recorded items of ``source`` (every recording of it, in case order)."""
    items: list[dict[str, Any]] = []
    for name in case.recordings:
        if name.split("/", 1)[0] == source:
            items += json.loads((RECORDINGS_DIR / name).read_text())["items"]
    return items


def _links(case: Case) -> list[tuple[tuple[str, str], tuple[str, str]]]:
    """The record pairs of two sources that carry one ``joinKey``."""
    primary = case.sources[0]
    pairs = []
    for other in case.sources[1:]:
        for left in _items(case, primary):
            for right in _items(case, other):
                if left["joinKey"] == right["joinKey"]:
                    pairs.append(((primary, left["id"]), (other, right["id"])))
    return sorted(set(pairs))


async def _write_links(client: MedullaClient, case: Case, run: str) -> None:
    """Write the approved ``owl:sameAs`` links through ``SourceIngest``."""
    primary = case.sources[0]
    connector = f"{primary}-{run}"
    provenance = {
        "connector": connector,
        "adapter_kind": "mcp",
        "server": primary,
        "tool": OPERATION,
        "tool_schema_sha256": "07" * 32,
        "source_uri": f"competency://{case.id}",
    }

    def record(stream: str, record_id: str) -> dict[str, Any]:
        return {
            "stream": stream,
            "record_id": record_id,
            "mapping_reference": f"connector:{connector}#mappings/link",
            "payload": {"id": record_id},
            "updated_at": "2026-11-26T00:00:00Z",
            "provenance": provenance,
        }

    records: list[dict[str, Any]] = []
    links: list[dict[str, Any]] = []
    for (left_stream, left_id), (right_stream, right_id) in _links(case):
        records += [record(left_stream, left_id), record(right_stream, right_id)]
        links.append(
            {
                "source": {"stream": left_stream, "record_id": left_id},
                "target": {"stream": right_stream, "record_id": right_id},
                "relation_reference": "owl:sameAs",
                "properties": {
                    "relationship": "owl:sameAs",
                    "approved_by": TEST_AGENT_ID,
                },
                "provenance": provenance,
            }
        )
    batch = {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": False,
        "records": copy.deepcopy(records),
        "relationships": links,
        "withdrawals": [],
        "provider_checkpoint": {
            "stream": primary,
            "position": {"page": 1},
            "watermark": "2026-11-26T00:00:00Z",
        },
        "expected_previous_checkpoint": None,
    }
    await client._send(
        "SourceIngest",
        {"request": batch},
        None,
        idempotency_key=f"competency-links:{case.id}:{run}",
    )


def _present(row: dict[str, Any]) -> set[str]:
    names = {"id"}
    for key in ("texts", "columns", "values"):
        part = row.get(key)
        if isinstance(part, dict):
            names |= set(part)
    return names


def _assert_rows(case: Case, answer: dict[str, Any]) -> None:
    assert answer["kind"] == "rows", answer
    rows = answer["rows"]
    assert len(rows) >= case.min_rows, (case.id, answer)
    connectors = {source for source in case.sources}
    for row in rows:
        provenance = row.get("provenance") or {}
        for field in ("connector", "contract_iri", "mapping_iri", "fetch_time_ms"):
            assert provenance.get(field), (field, row)
        assert any(provenance["connector"].startswith(f"{s}-") for s in connectors), row
    seen: set[str] = set()
    for row in rows:
        seen |= _present(row)
    missing = [c for c in case.columns if c not in seen]
    assert not missing, (case.id, missing, answer)
    covered = {entry["source"]: entry for entry in answer.get("source_coverage", [])}
    for source in case.sources:
        assert covered.get(source, {}).get("used") is True, (source, answer)


ONTOLOGY_DIR = (
    Path(__file__).resolve().parents[2] / "crates" / "medulla-core" / "ontology"
)


def _term_iri(module: str, term: str) -> str:
    """The IRI a module declares for ``term`` (its prefix resolved from the file)."""
    text = (ONTOLOGY_DIR / module).read_text()
    prefixes = dict(re.findall(r"^@prefix (\w*): <([^>]+)> \.", text, re.M))
    declared = re.search(rf"^(\w*):{re.escape(term)}\s+a\s", text, re.M)
    assert declared, f"{module} declares no term {term}"
    return prefixes[declared.group(1)] + term


async def _terms_resolve(client: MedullaClient, case: Case) -> None:
    """Every term resolves in its module; the same local name elsewhere does not."""
    for term in case.terms:
        iri = _term_iri(case.vocabulary_module, term)
        found = solution_values(
            await client.rdf.sparql(f"SELECT ?kind WHERE {{ <{iri}> a ?kind }}")
        )
        assert found, f"{iri} does not resolve in {case.vocabulary_module}"
        base = iri.rpartition("#")[0]
        foreign = f"{base}-another-module#{term}"
        none = solution_values(
            await client.rdf.sparql(f"SELECT ?kind WHERE {{ <{foreign}> a ?kind }}")
        )
        assert not none, f"{term} must not resolve in another module"


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.9")
def test_cases_exist() -> None:
    """An empty cases directory is never a silent pass."""
    assert CASES, "load_cases() returned no case"


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.9")
@pytest.mark.parametrize("case", _params())
def test_case(case: Case, clean_graph, tmp_path: Path) -> None:
    run = uuid.uuid4().hex[:8]
    before = stored_nodes(clean_graph)
    answers: list[dict[str, Any]] = []
    fixtures: list[FixtureSource] = []

    async def exercise() -> None:
        client = await _connect()
        try:
            digests = await _prepare(client, case)
            executions: dict[str, dict[str, Any]] = {}
            for source in case.sources:
                execution, fixture = await _read(
                    client, case, tmp_path, source, run, digests
                )
                executions[source] = execution
                fixtures.append(fixture)
            if case.kind == "cross":
                await _write_links(client, case, run)
            answers.append(
                await client.query.uql(
                    case.uql, foreign_executions=list(executions.values())
                )
            )
            await _terms_resolve(client, case)
        finally:
            await client.close()

    try:
        asyncio.run(exercise())
    finally:
        for fixture in fixtures:
            fixture.close()

    _assert_rows(case, answers[0])
    after = stored_nodes(clean_graph)
    if case.kind == "single":
        assert after == before, "the store holds no row of the source after the read"
    else:
        added = sorted(set(after) - set(before))
        linked = {i for pair in _links(case) for _, i in pair}
        assert all(any(node.endswith(i) for i in linked) for node in added), added


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.9")
@pytest.mark.parametrize("case", _params())
def test_cross_tenant_case_returns_no_row(
    case: Case, clean_graph, tmp_path: Path
) -> None:
    """The statement read under a second tenant returns no row."""
    run = uuid.uuid4().hex[:8]
    outcomes: list[Any] = []
    fixtures: list[FixtureSource] = []

    async def exercise() -> None:
        owner = await _connect()
        other = await _connect(tenant_request_context(OTHER_TENANT))
        try:
            digests = await _prepare(owner, case)
            executions = []
            for source in case.sources:
                execution, fixture = await _read(
                    owner, case, tmp_path, source, run, digests
                )
                executions.append(execution)
                fixtures.append(fixture)
            try:
                outcomes.append(
                    await other.query.uql(case.uql, foreign_executions=executions)
                )
            except Exception as refusal:  # the engine names the refusal
                outcomes.append(refusal)
        finally:
            await owner.close()
            await other.close()

    try:
        asyncio.run(exercise())
    finally:
        for fixture in fixtures:
            fixture.close()

    outcome = outcomes[0]
    if not isinstance(outcome, Exception):
        assert outcome.get("rows", []) == [], outcome


def _victim(case: Case) -> str:
    return case.sources[-1]


def _assert_gap(outcome: Any, victim: str, codes: tuple[str, ...]) -> None:
    """The answer abstains on ``victim`` by name; it never returns a wrong answer."""
    if isinstance(outcome, Exception):
        text = str(outcome)
        assert any(code in text for code in codes + (victim,)), text
        return
    for row in outcome.get("rows", []):
        provenance = row.get("provenance") or {}
        assert not str(provenance.get("connector", "")).startswith(f"{victim}-"), row
    entries = {e["source"]: e for e in outcome.get("source_coverage", [])}
    if entries:
        assert entries[victim]["used"] is False, outcome
    else:
        assert not outcome.get("rows"), outcome


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.10")
@pytest.mark.parametrize(("case", "variant"), _params(VARIANTS))
def test_case_abstains(case: Case, variant: str, clean_graph, tmp_path: Path) -> None:
    """A source absent, held or stopped abstains by name; never a wrong answer."""
    run = uuid.uuid4().hex[:8]
    victim = _victim(case)
    outcomes: list[Any] = []
    fixtures: list[FixtureSource] = []

    async def exercise() -> None:
        client = await _connect()
        try:
            digests = await _prepare(client, case)
            executions = []
            for source in case.sources:
                if source != victim:
                    execution, fixture = await _read(
                        client, case, tmp_path, source, run, digests
                    )
                    executions.append(execution)
                    fixtures.append(fixture)
            if variant == "held":
                with pytest.raises(Exception) as held:
                    _, fixture = await _read(
                        client, case, tmp_path, victim, run, digests, approve=False
                    )
                    fixtures.append(fixture)
                assert any(code in str(held.value) for code in HELD_CODES), held.value
            if variant == "stopped":
                fixture = _fixture(tmp_path, case, victim)
                fixtures.append(fixture)
                fixture.stop()
                status, _ = _host_fetch(fixture, case.source_kinds[victim])
                assert status == 503
                assert _named_refusal(status) == UNAVAILABLE_REFUSAL
            try:
                outcomes.append(
                    await client.query.uql(case.uql, foreign_executions=executions)
                )
            except Exception as refusal:  # the engine names the abstention
                outcomes.append(refusal)
        finally:
            await client.close()

    try:
        asyncio.run(exercise())
    finally:
        for fixture in fixtures:
            fixture.close()

    codes = ABSENT_MARKERS if variant == "absent" else ABSENT_MARKERS + HELD_CODES
    _assert_gap(outcomes[0], victim, codes)
