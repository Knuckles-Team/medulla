"""Served `OntologySlice`: MEDULLA-INGEST-WORKER-R913.1 to R913.4.

The slice of one text unit holds the classes of the graph's admitted ontology that
match it, capped at `max_classes`, with the properties connected to them. An ontology
of `max_classes` classes or fewer is returned whole. The method needs `source:read`.
"""

from __future__ import annotations

import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import TEST_AGENT_ID, TEST_SIGNER_KEY, request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries, ontology_entry

from medulla.client import EngineResponseError, MedullaClient, SyncMedullaClient

from .test_source_stream_versions_served import _close, _connector, _declaration

NS = "http://example.org/o#"


def _ontology(count: int) -> str:
    """`count` classes, each with a label, a comment holding its own marker word and
    a property from it to the next class."""
    lines = [
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .",
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
        f"@prefix ex: <{NS}> .",
    ]
    for n in range(count):
        lines.append(
            f'ex:C{n:03d} a owl:Class ; rdfs:label "Species {n:03d}" ; '
            f'rdfs:comment "Describes tokenmarker{n:03d} in the field." .'
        )
        lines.append(
            f"ex:rel{n:03d} rdfs:domain ex:C{n:03d} ; "
            f"rdfs:range ex:C{(n + 1) % count:03d} ."
        )
    return "\n".join(lines)


async def _graph_with_ontology(count: int) -> tuple[MedullaClient, str]:
    connector = _connector()
    admin = await connect()
    graph = f"ontology-slice-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(
            admin,
            connector,
            [
                _declaration(connector),
                ontology_entry(connector, "species", _ontology(count)),
            ],
        )
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


async def _slice(client: MedullaClient, **request: Any) -> dict[str, Any]:
    answer = await client._send("OntologySlice", {"request": request})
    assert isinstance(answer, dict)
    return answer


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R913.4")
@pytest.mark.asyncio
async def test_slice_selects_by_similarity_and_bypasses_small_ontology() -> None:
    large, large_graph = await _graph_with_ontology(200)
    small, small_graph = await _graph_with_ontology(5)
    try:
        unit = "A paragraph about tokenmarker137 seen in the field."
        answer = await _slice(large, unit_text=unit)
        iris = [item["iri"] for item in answer["classes"]]
        assert f"{NS}C137" in iris
        assert len(iris) <= 32
        assert answer["bypassed"] is False
        # The property that starts at the returned class is connected to it.
        assert f"{NS}rel137" in [item["iri"] for item in answer["properties"]]
        # The same unit gives the same slice.
        assert await _slice(large, unit_text=unit) == answer

        whole = await _slice(small, unit_text=unit)
        assert whole["bypassed"] is True
        assert len(whole["classes"]) == 5
        assert len(whole["properties"]) == 5
    finally:
        await _close(large, large_graph)
        await _close(small, small_graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R913.2")
@pytest.mark.asyncio
async def test_each_refusal_names_its_code() -> None:
    client, graph = await _graph_with_ontology(40)
    empty = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=f"ontology-slice-empty-{uuid.uuid4().hex[:8]}",
    )
    try:
        with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
            await _slice(client, unit_text="  ")
        with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
            await _slice(client, unit_text="x" * 65_537)
        with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
            await _slice(client, unit_text="tokenmarker001", max_classes=0)
        with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
            await _slice(client, unit_text="tokenmarker001", max_classes=257)
        with pytest.raises(EngineResponseError, match="ONTOLOGY_NOT_ADMITTED"):
            await _slice(empty, unit_text="tokenmarker001")
    finally:
        await empty.close()
        await _close(client, graph)


def _register(graph: str, agent_id: str, role: str) -> None:
    """Register `agent_id` as an Agent holding `role`, with graph Read."""
    admin = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        try:
            admin.rbac.add_role(role)
        except EngineResponseError:
            pass  # another case on this engine already created it
        admin.rbac.add_grant(role, {"Graph": graph}, "Read")
        admin.consensus.register_identity(
            agent_id,
            "Agent",
            [],
            [role],
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
    finally:
        admin.close()


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R913.3")
@pytest.mark.asyncio
async def test_method_is_registered_with_read_action() -> None:
    client, graph = await _graph_with_ontology(40)
    suffix = uuid.uuid4().hex[:8]
    agent = f"service:slice-{suffix}"
    readers: list[MedullaClient] = []
    try:
        _register(graph, agent, f"suite:slice-{suffix}")
        for scopes, allowed in [
            (["kg:read"], False),
            (["kg:read", "source:read"], True),
        ]:
            reader = await MedullaClient.connect(
                socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
                verified_context=request_context(agent_id=agent, scopes=scopes),
                graph_name=graph,
            )
            readers.append(reader)
            if allowed:
                answer = await _slice(reader, unit_text="tokenmarker007")
                assert f"{NS}C007" in [item["iri"] for item in answer["classes"]]
            else:
                with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
                    await _slice(reader, unit_text="tokenmarker007")
    finally:
        for reader in readers:
            await reader.close()
        await _close(client, graph)
