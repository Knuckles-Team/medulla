"""Served proof for MEDULLA-DURABLE-KERNEL-R877: a knowledge bundle moves one scoped
dataset between two tenants.

Tenant A holds one graph with a document node, an embedding and a stored source
document. A exports it as a bundle, the bundle blob moves to tenant B through the blob
methods, and B imports it under a prefix. B then answers the same UQL query, the same
vector search and the same document read as A. A repeat import into the existing graph
refuses ``BUNDLE_GRAPH_EXISTS``, and a bundle with one changed byte refuses
``BUNDLE_DIGEST_MISMATCH`` and leaves no graph behind.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context, tenant_request_context

from medulla.client import MedullaClient

pytestmark = [pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R877")]

COMMONS = "__commons__"
CAS = "cas:sha256:"
DOCUMENT = b"the source document of the bundle"
TOKEN = "alpha-token"


async def _connect(context: dict[str, object], graph: str) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=graph,
        timeout=60,
    )


async def _journey(suffix: str) -> dict[str, Any]:
    graph_a = f"bundle-a-{suffix}"
    prefix = f"imp-{suffix}-"
    target = f"{prefix}{graph_a}"
    tenant_b = f"tenant:bundle-b-{suffix}"
    admin = await _connect(request_context(), COMMONS)
    client_a = await _connect(request_context(), graph_a)
    client_b = await _connect(tenant_request_context(tenant_b), COMMONS)
    imported = None
    try:
        await admin.tenants.create(graph_a, "Global")
        digest = await client_a.blob.store(DOCUMENT)
        await client_a.lifecycle.batch_update(
            [
                {
                    "op": "upsert_node",
                    "id": "doc-1",
                    "properties": {
                        "type": "Doc",
                        "title": TOKEN,
                        "content_ref": f"{CAS}{digest}",
                    },
                }
            ]
        )
        await client_a._send(
            "AddEmbedding", {"node_id": "doc-1", "embedding": [1.0, 0.0, 0.0]}
        )

        receipt = await client_a._send(
            "BundleExport",
            {
                "graphs": [graph_a],
                "include_embeddings": True,
                "include_documents": True,
            },
        )
        assert receipt["bundle_ref"].startswith(CAS)
        archive = await client_a.blob.fetch(receipt["bundle_ref"].removeprefix(CAS))
        assert len(archive) == receipt["size_bytes"]

        # The blob moves to tenant B through the blob methods.
        ref_b = CAS + await client_b.blob.store(archive)
        imported = await client_b._send(
            "BundleImport",
            {
                "bundle_ref": ref_b,
                "target_graph_prefix": prefix,
                "on_conflict": "refuse",
            },
        )

        client_target = await _connect(tenant_request_context(tenant_b), target)
        try:
            query = "MATCH (:Doc) |> RETURN title"
            rows_a = await client_a.query.uql(query)
            rows_b = await client_target.query.uql(query)
            hits_a = await client_a._send(
                "SemanticSearch", {"query_embedding": [1.0, 0.0, 0.0], "n_results": 3}
            )
            hits_b = await client_target._send(
                "SemanticSearch", {"query_embedding": [1.0, 0.0, 0.0], "n_results": 3}
            )
            props = await client_target._send("GetNodeProperties", {"node_id": "doc-1"})
            moved = await client_target.blob.fetch(
                _content_ref(props).removeprefix(CAS)
            )
        finally:
            await client_target.close()

        # A second import into the existing graph refuses, writing nothing.
        again = None
        try:
            await client_b._send(
                "BundleImport",
                {
                    "bundle_ref": ref_b,
                    "target_graph_prefix": prefix,
                    "on_conflict": "refuse",
                },
            )
        except Exception as error:
            again = str(error)

        # One changed byte of a part refuses and leaves no graph behind.
        damaged = archive.replace(TOKEN.encode(), b"alpha-tokeX", 1)
        assert damaged != archive and len(damaged) == len(archive)
        bad_ref = CAS + await client_b.blob.store(damaged)
        broken = None
        try:
            await client_b._send(
                "BundleImport",
                {
                    "bundle_ref": bad_ref,
                    "target_graph_prefix": f"bad-{suffix}-",
                    "on_conflict": "refuse",
                },
            )
        except Exception as error:
            broken = str(error)
        listed = await client_b.tenants.list()
        return {
            "imported": imported,
            "rows_a": rows_a,
            "rows_b": rows_b,
            "hits_a": hits_a,
            "hits_b": hits_b,
            "moved": moved,
            "again": again,
            "broken": broken,
            "graphs": [name for name, _ in listed],
            "bad_graph": f"bad-{suffix}-{graph_a}",
        }
    finally:
        for client in (client_b, client_a, admin):
            await client.close()


def _content_ref(properties: Any) -> str:
    row = properties[0] if isinstance(properties, list) else properties
    if isinstance(row, (bytes, bytearray)):
        import msgpack

        row = msgpack.unpackb(row, raw=False)
    return str(row["content_ref"])


def _ids(hits: Any) -> list[str]:
    return [str(hit[0]) for hit in hits]


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R877.5")
def test_export_import_round_trip_between_tenants(clean_graph):
    # spec: MEDULLA-DURABLE-KERNEL-R877.5
    out = asyncio.run(_journey(uuid.uuid4().hex[:8]))
    imported = out["imported"]
    assert imported["triples"] == 0 and imported["provenance"] >= 1
    assert imported["embeddings"] == 1 and imported["documents"] == 1
    assert [row["skipped"] for row in imported["graphs"]] == [False]

    # Tenant B answers what tenant A answers.
    assert out["rows_b"] == out["rows_a"]
    assert _ids(out["hits_b"]) == _ids(out["hits_a"]) == ["doc-1"]
    assert out["moved"] == DOCUMENT

    # A refusal names its code and writes nothing.
    assert out["again"] is not None and "BUNDLE_GRAPH_EXISTS" in out["again"]
    assert out["broken"] is not None and "BUNDLE_DIGEST_MISMATCH" in out["broken"]
    assert out["bad_graph"] not in out["graphs"]
