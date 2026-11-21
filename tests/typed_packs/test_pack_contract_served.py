"""Served release contract of a connector pack (MEDULLA-TYPED-PACKS-R107.3, R107.4).

Through the served ``ConnectorPack.Import``: the release a pack would activate is
diffed against the one the head activated before anything commits. An additive
release commits and becomes the active one. A release that removes a class is
held behind a change request and answers ``CONTRACT_BREAK_PENDING`` with its
breaks; one that removes a class stored data still names answers
``CLASS_IN_USE``. In both refusals the head, and so the active version, is
unchanged.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context
from ontology_pack_support import (
    attach_pack,
    connect,
    import_entries,
    ontology_entry,
    violation_codes,
)

from medulla import ConnectorPackClient
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackHeadRef

CORE = "https://knuckles-team.github.io/ontology/core"
WORK_ITEM = f"{CORE}#WorkItem"
HEADER = (
    "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
    "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
)


def _ontology(base: str, classes: list[str]) -> str:
    """Each named class is a direct subclass of the core work item."""
    return HEADER + "".join(
        f"<{base}#{name}> a owl:Class ; rdfs:subClassOf <{WORK_ITEM}> .\n"
        for name in classes
    )


def _head(receipt: dict) -> PackHeadRef:
    return PackHeadRef(
        binding_revision=receipt["binding_revision"],
        pack_digest=receipt["pack_digest"],
    )


async def _status_head(admin: MedullaClient, connector: str) -> tuple[int, str]:
    status = await ConnectorPackClient(admin).status(
        tenant_id=TEST_TENANT, connector=connector
    )
    assert status.head is not None
    return status.head.binding_revision, str(status.head.pack_digest)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R107.3")
def test_diff_runs_in_import_before_commit() -> None:
    async def exercise() -> None:
        connector = f"contract-diff-{uuid.uuid4().hex[:8]}"
        base = f"https://example.org/{connector}"
        entry = lambda classes: [  # noqa: E731
            ontology_entry(connector, "vocabulary", _ontology(base, classes))
        ]
        admin = await connect()
        try:
            first = await import_entries(
                admin, connector, entry(["Ticket", "Incident"])
            )
            assert first["result"] == "imported", first
            # An additive release applies at once and becomes the active one.
            second = await import_entries(
                admin,
                connector,
                entry(["Ticket", "Incident", "Epic"]),
                expected_head=_head(first["receipt"]),
            )
            assert second["result"] == "imported", second
            active = await _status_head(admin, connector)
            assert active[0] == second["receipt"]["binding_revision"]

            # A release that drops a class is diffed before the commit: it
            # never reaches the head, so the active release is still the second.
            breaking = await import_entries(
                admin,
                connector,
                entry(["Ticket", "Epic"]),
                expected_head=_head(second["receipt"]),
            )
            assert breaking["result"] == "rejected", breaking
            assert await _status_head(admin, connector) == active

            # The refusal held the release: the head accepts the next additive
            # release against the unchanged head.
            third = await import_entries(
                admin,
                connector,
                entry(["Ticket", "Incident", "Epic", "Story"]),
                expected_head=_head(second["receipt"]),
            )
            assert third["result"] == "imported", third
        finally:
            await admin.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R107.4")
def test_breaking_and_in_use_refuse() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"contract-break-{tag}"
        base = f"https://example.org/{connector}"
        graph = f"contract-break-{tag}"
        entry = lambda classes: [  # noqa: E731
            ontology_entry(connector, "vocabulary", _ontology(base, classes))
        ]
        admin = await connect()
        try:
            first = await import_entries(
                admin, connector, entry(["Ticket", "Incident"])
            )
            assert first["result"] == "imported", first
            head = _head(first["receipt"])
            active = await _status_head(admin, connector)

            # No data names the removed class: the release is held, with its break.
            pending = await import_entries(
                admin, connector, entry(["Ticket"]), expected_head=head
            )
            assert violation_codes(pending) == {"CONTRACT_BREAK_PENDING"}, pending
            assert any(f"{base}#Incident" == v["uri"] for v in pending["violations"]), (
                pending
            )
            assert pending["budget_exhausted"] is False, pending
            assert await _status_head(admin, connector) == active

            # A graph composing the head holds a fact typed with the class.
            await admin.tenants.create(graph)
        finally:
            await admin.close()

        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            graph_name=graph,
        )
        try:
            await attach_pack(client, connector)
            await client.rdf.add_triples(
                turtle=(
                    f"@prefix ex: <{base}#> .\n"
                    "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
                    "<https://example.org/data/case-1> rdf:type ex:Incident .\n"
                )
            )
            admin = await connect()
            try:
                in_use = await import_entries(
                    admin, connector, entry(["Ticket"]), expected_head=head
                )
                assert violation_codes(in_use) == {"CLASS_IN_USE"}, in_use
                assert [v["uri"] for v in in_use["violations"]] == [f"{base}#Incident"]
                assert await _status_head(admin, connector) == active
            finally:
                await admin.close()
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())
