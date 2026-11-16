"""MEDULLA-TYPED-PACKS-R097.701 served acceptance: domain pack refusals and evidence.

Drives the served ``DomainPack`` method through the real socket:

* ``preview`` proposes facts and writes nothing;
* ``run`` promotes the same facts as one durable batch, and every promoted node
  records the pack id, version, manifest integrity digest and source artifact;
* a tampered manifest, a mapping with no declared target and a mapping onto a
  class the pack does not declare each refuse with their own registered code
  and leave the graph exactly as it was.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import graph as graph_methods


def _integrity(manifest: dict[str, Any]) -> str:
    """The digest the engine takes over the manifest body: the serde field order,
    compact separators, every field but the digest itself."""
    body = {
        key: manifest[key]
        for key in ("schema_version", "pack_id", "version", "classes", "mappings")
    }
    body["cases"] = manifest["cases"]
    encoded = json.dumps(body, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def _manifest(pack_id: str, *, target_class: str = "Person", target: str = "name"):
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "pack_id": pack_id,
        "version": "1",
        "integrity_sha256": "",
        "classes": [{"name": "Person", "properties": ["name"], "relations": []}],
        "mappings": [
            {
                "source_kind": "column",
                "selector": "full_name",
                "target_class": target_class,
                "target_property_or_relation": target,
            }
        ],
        "cases": [],
    }
    manifest["integrity_sha256"] = _integrity(manifest)
    return manifest


def _input(manifest: dict[str, Any]) -> dict[str, Any]:
    return {
        "manifest": manifest,
        "rows": [
            {
                "row_id": "r1",
                "source_kind": "column",
                "fields": {"full_name": "Ada"},
            }
        ],
        "markdown": [],
    }


async def _send(client: MedullaClient, op: str, manifest: dict[str, Any]) -> Any:
    result = await graph_methods.send_domain_pack(
        client, {"op": {"op": op, "input": _input(manifest)}}
    )
    return result.payload


async def _fresh_graph() -> tuple[MedullaClient, MedullaClient, str]:
    admin = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    graph = f"domain-pack-{uuid.uuid4().hex[:8]}"
    await admin.tenants.create(graph)
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return admin, client, graph


async def _refusal(client: MedullaClient, op: str, manifest: dict[str, Any]) -> str:
    with pytest.raises(EngineResponseError) as refused:
        await _send(client, op, manifest)
    return refused.value.code


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.701")
def test_preview_then_run_promotes_only_through_envelope() -> None:
    async def exercise() -> None:
        admin, client, graph = await _fresh_graph()
        try:
            pack_id = f"hr-{uuid.uuid4().hex[:6]}"
            manifest = _manifest(pack_id)
            before = await client.nodes.count()

            preview = await _send(client, "preview", manifest)
            assert preview["pack_id"] == pack_id
            assert any(fact["fact"] == "entity" for fact in preview["facts"])
            assert await client.nodes.count() == before, "preview writes nothing"

            tampered = {**manifest, "version": "2"}
            unknown = _manifest(pack_id, target_class="Ghost")
            undeclared = _manifest(pack_id, target="undeclared")
            for op in ("preview", "run"):
                assert (
                    await _refusal(client, op, tampered)
                    == "DOMAIN_PACK_INTEGRITY_MISMATCH"
                )
                assert (
                    await _refusal(client, op, unknown) == "DOMAIN_PACK_CLASS_UNKNOWN"
                )
                assert (
                    await _refusal(client, op, undeclared)
                    == "DOMAIN_PACK_MAPPING_INVALID"
                )
            assert await client.nodes.count() == before, "a refusal writes nothing"

            run = await _send(client, "run", manifest)
            assert run["entities_written"] == 1
            assert await client.nodes.count() == before + 1
            node = await client.nodes.properties(f"{pack_id}:Person:r1")
            assert node is not None
            assert node["domain_pack"] == pack_id
            assert node["domain_pack_version"] == "1"
            assert node["domain_pack_integrity_sha256"] == manifest["integrity_sha256"]
            assert node["domain_pack_source"] == "r1"
            assert node["name"] == "Ada"
        finally:
            await client.close()
            await admin.tenants.delete(graph)
            await admin.close()

    asyncio.run(exercise())
