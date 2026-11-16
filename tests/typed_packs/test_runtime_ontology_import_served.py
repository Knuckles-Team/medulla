"""Served runtime ontology hot path (MEDULLA-TYPED-PACKS-R103.6).

A connector imports its ontology through ``GraphSchema``; a served SPARQL query
on the same graph sees the imported class after the import and after a revision,
and no longer sees it after ``WithdrawOntology``. A second principal cannot
revise or withdraw it (``IMPORTER_MISMATCH``), and a second graph never sees it.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import request_context
from ontology_pack_support import connect

from medulla.client import EngineResponseError, MedullaClient

HEADER = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"


def class_body(iri: str, *names: str) -> str:
    return HEADER + "".join(f"<{iri}#{name}> a owl:Class .\n" for name in names)


async def tenant_sees_class(tenant: str, iri: str, name: str) -> bool:
    """Whether a client of another tenant reads the class; a refusal reads nothing."""
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context={**request_context(), "tenant": tenant},
    )
    try:
        return await sees_class(client, iri, name)
    except EngineResponseError:
        return False
    finally:
        await client.close()


async def sees_class(client: MedullaClient, iri: str, name: str) -> bool:
    rows = await client.rdf.sparql(f"SELECT ?p ?o WHERE {{ <{iri}#{name}> ?p ?o }}")
    return bool(rows)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R103.6")
def test_import_revise_withdraw_end_to_end() -> None:
    async def exercise() -> None:
        owner = await connect()
        other_graph = f"ontology-other-{uuid.uuid4().hex[:8]}"
        intruder = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(principal="ontology-intruder"),
        )
        try:
            iri = f"http://example.org/runtime/{uuid.uuid4().hex}"
            connector = "runtime-ontology"
            assert not await sees_class(owner, iri, "Alpha")

            first = await owner.rdf.import_ontology(
                connector,
                iri,
                "1",
                class_body(iri, "Alpha"),
                idempotency_key=f"import:{uuid.uuid4().hex}",
            )
            assert first["revision"] == 1
            assert await sees_class(owner, iri, "Alpha")
            assert not await sees_class(owner, iri, "Beta")

            # A second principal can neither revise nor withdraw it.
            with pytest.raises(EngineResponseError, match="IMPORTER_MISMATCH"):
                await intruder.rdf.import_ontology(
                    connector,
                    iri,
                    "2",
                    class_body(iri, "Alpha", "Beta"),
                    idempotency_key=f"import:{uuid.uuid4().hex}",
                )
            with pytest.raises(EngineResponseError, match="IMPORTER_MISMATCH"):
                await intruder.rdf.withdraw_ontology(
                    connector,
                    iri,
                    1,
                    idempotency_key=f"withdraw:{uuid.uuid4().hex}",
                )
            assert not await sees_class(owner, iri, "Beta")

            # The same digest answers unchanged; a new digest is the next revision.
            again = await owner.rdf.import_ontology(
                connector,
                iri,
                "1",
                class_body(iri, "Alpha"),
                idempotency_key=f"import:{uuid.uuid4().hex}",
            )
            assert again["revision"] == 1
            second = await owner.rdf.import_ontology(
                connector,
                iri,
                "2",
                class_body(iri, "Alpha", "Beta"),
                idempotency_key=f"import:{uuid.uuid4().hex}",
            )
            assert second["revision"] == 2
            assert await sees_class(owner, iri, "Beta")

            # A second tenant never sees the import.
            assert not await tenant_sees_class("tenant-b", iri, "Alpha")

            # A second graph of the same tenant never sees the import.
            await owner.tenants.create(other_graph)
            elsewhere = await MedullaClient.connect(
                socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
                verified_context=request_context(),
                graph_name=other_graph,
            )
            try:
                assert not await sees_class(elsewhere, iri, "Alpha")
            finally:
                await elsewhere.close()
                await owner.tenants.delete(other_graph)

            await owner.rdf.withdraw_ontology(
                connector,
                iri,
                2,
                idempotency_key=f"withdraw:{uuid.uuid4().hex}",
            )
            assert not await sees_class(owner, iri, "Alpha")
            assert not await sees_class(owner, iri, "Beta")
        finally:
            await intruder.close()
            await owner.close()

    asyncio.run(exercise())
