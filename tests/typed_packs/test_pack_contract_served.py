"""Served release contract of a connector pack (MEDULLA-TYPED-PACKS-R107.3, R107.4).

Through the served ``ConnectorPack.Import``: the release a pack would activate is
diffed against the one the head activated before anything commits. An additive
release commits and becomes the active one. A release that removes a class is
held behind a change request and answers ``CONTRACT_BREAK_PENDING`` with its
breaks; one that removes a class stored data still names answers
``CLASS_IN_USE``. In both refusals the head, and so the active version, is
unchanged. Only the importing tenant's graphs count: another tenant's data in
the same store never holds back a withdrawal.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    find_server_binary,
    request_context,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)
from ontology_pack_support import (
    attach_pack,
    connect,
    import_entries,
    ontology_entry,
    violation_codes,
)

from medulla import ConnectorPackClient
from medulla.client import MedullaClient, SyncMedullaClient
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R107.5")
def test_receipt_carries_digests_break_count_and_request() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"contract-receipt-{tag}"
        base = f"https://example.org/{connector}"
        entry = lambda classes: [  # noqa: E731
            ontology_entry(connector, "vocabulary", _ontology(base, classes))
        ]
        admin = await connect()
        try:
            # A first release has no previous digest and no breaks.
            first = await import_entries(
                admin, connector, entry(["Ticket", "Incident"])
            )
            assert first["result"] == "imported", first
            first_release = first["receipt"]["release"]
            assert first_release["previous_contract_digest"] is None
            assert first_release["contract_digest"].startswith("sha256:")
            assert first_release["break_count"] == 0
            assert first_release.get("change_request") is None

            # An additive re-import names both contract digests and no breaks.
            second = await import_entries(
                admin,
                connector,
                entry(["Ticket", "Incident", "Epic"]),
                expected_head=_head(first["receipt"]),
            )
            assert second["result"] == "imported", second
            second_release = second["receipt"]["release"]
            assert (
                second_release["previous_contract_digest"]
                == first_release["contract_digest"]
            )
            assert second_release["contract_digest"] != first_release["contract_digest"]
            assert second_release["break_count"] == 0
            assert second_release.get("change_request") is None

            # A breaking re-import is held: its answer carries the diff and the request.
            breaking = await import_entries(
                admin,
                connector,
                entry(["Ticket", "Epic"]),
                expected_head=_head(second["receipt"]),
            )
            assert breaking["result"] == "rejected", breaking
            held = breaking["held"]
            assert held["break_count"] >= 1
            assert held["previous_contract_digest"] == second_release["contract_digest"]
            assert held["contract_digest"].startswith("sha256:")
            assert held["change_request"]["target"] == f"pack:{connector}"
            assert held["change_request"]["proposed_digest"].startswith("sha256:")
        finally:
            await admin.close()

    asyncio.run(exercise())


OTHER_TENANT = "tenant:other"


def _tenant_context(tenant: str, base: dict[str, object]) -> dict[str, object]:
    return {**base, "tenant": tenant}


@contextmanager
def _engine(
    runtime: Path, persist: Path, tenant: str, *, bootstrap: bool
) -> Iterator[str]:
    """One served engine for `tenant` over the shared store in `persist`.

    The door admits one tenant per process (``MEDULLA_TENANT``), so the two
    tenants of one store are served one after the other by engines that open
    the same durable directory.
    """

    binary = find_server_binary()
    if binary is None:
        pytest.fail("no full-featured medulla-server binary is built")
    socket_path = str(runtime / f"{tenant.replace(':', '-')}.sock")
    log_path = runtime / f"{tenant.replace(':', '-')}.log"
    env = {
        **os.environ,
        **strict_server_env(
            str(runtime / "security"),
            auth_secret=os.environ["GRAPH_SERVICE_AUTH_SECRET"],
            persist_dir=str(persist),
        ),
        "MEDULLA_TENANT": tenant,
    }
    process = spawn_server(
        [binary, "--socket-path", socket_path], env=env, log_path=log_path
    )
    try:
        wait_for_server(
            process, socket_path, name=f"{tenant} engine", log_path=log_path
        )
        if bootstrap:
            system = SyncMedullaClient.connect(
                socket_path=socket_path,
                verified_context=_tenant_context(tenant, bootstrap_context()),
            )
            try:
                system.consensus.bootstrap_system_identity(
                    agent_id=TEST_AGENT_ID,
                    signer_id=TEST_AGENT_ID,
                    signer_key=TEST_SIGNER_KEY,
                )
            finally:
                system.close()
        yield socket_path
    finally:
        stop_server(process, name=f"{tenant} engine", log_path=log_path)


async def _store_typed_fact(
    socket_path: str, tenant: str, connector: str, graph: str, iri: str, cls: str
) -> None:
    """Create `graph`, compose the tenant's pack into it and type `iri` as `cls`."""

    context = _tenant_context(tenant, request_context())
    admin = await MedullaClient.connect(
        socket_path=socket_path, verified_context=context
    )
    try:
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=socket_path, verified_context=context, graph_name=graph
    )
    try:
        await attach_pack(client, connector)
        await client.rdf.add_triples(
            turtle=(
                "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
                f"<{iri}> rdf:type <{cls}> .\n"
            )
        )
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R107.4")
def test_other_tenant_data_never_blocks_a_withdrawal(tmp_path: Path) -> None:
    """Two tenants import the same connector into one store. Tenant B's fact
    typed with a class tenant A withdraws leaves A's release held as a break,
    not refused as in use; A's own fact typed with it refuses the release."""

    tag = uuid.uuid4().hex[:8]
    connector = f"contract-tenants-{tag}"
    base = f"https://example.org/{connector}"
    incident = f"{base}#Incident"
    entry = lambda classes: [  # noqa: E731
        ontology_entry(connector, "vocabulary", _ontology(base, classes))
    ]
    persist = tmp_path / "persist"
    persist.mkdir()

    async def other_tenant(socket_path: str) -> None:
        context = _tenant_context(OTHER_TENANT, request_context())
        admin = await MedullaClient.connect(
            socket_path=socket_path, verified_context=context
        )
        try:
            first = await import_entries(
                admin, connector, entry(["Ticket", "Incident"])
            )
            assert first["result"] == "imported", first
        finally:
            await admin.close()
        await _store_typed_fact(
            socket_path,
            OTHER_TENANT,
            connector,
            f"contract-other-{tag}",
            "https://example.org/data/other-case",
            incident,
        )

    async def importing_tenant(socket_path: str) -> None:
        context = _tenant_context(TEST_TENANT, request_context())
        admin = await MedullaClient.connect(
            socket_path=socket_path, verified_context=context
        )
        try:
            first = await import_entries(
                admin, connector, entry(["Ticket", "Incident"])
            )
            assert first["result"] == "imported", first
            head = _head(first["receipt"])
            status = await ConnectorPackClient(admin).status(
                tenant_id=TEST_TENANT, connector=connector
            )
            assert status.head is not None
            active = (status.head.binding_revision, str(status.head.pack_digest))

            # The other tenant's stored fact names the class; the withdrawal
            # is still only a held break.
            pending = await import_entries(
                admin, connector, entry(["Ticket"]), expected_head=head
            )
            assert violation_codes(pending) == {"CONTRACT_BREAK_PENDING"}, pending
            assert any(incident == v["uri"] for v in pending["violations"]), pending
        finally:
            await admin.close()

        await _store_typed_fact(
            socket_path,
            TEST_TENANT,
            connector,
            f"contract-own-{tag}",
            "https://example.org/data/own-case",
            incident,
        )
        admin = await MedullaClient.connect(
            socket_path=socket_path, verified_context=context
        )
        try:
            # The importing tenant's own fact refuses the same release.
            in_use = await import_entries(
                admin, connector, entry(["Ticket"]), expected_head=head
            )
            assert violation_codes(in_use) == {"CLASS_IN_USE"}, in_use
            assert [v["uri"] for v in in_use["violations"]] == [incident], in_use
            status = await ConnectorPackClient(admin).status(
                tenant_id=TEST_TENANT, connector=connector
            )
            assert status.head is not None
            assert (
                status.head.binding_revision,
                str(status.head.pack_digest),
            ) == active
        finally:
            await admin.close()

    with _engine(tmp_path, persist, OTHER_TENANT, bootstrap=True) as socket_path:
        asyncio.run(other_tenant(socket_path))
    with _engine(tmp_path, persist, TEST_TENANT, bootstrap=False) as socket_path:
        asyncio.run(importing_tenant(socket_path))
