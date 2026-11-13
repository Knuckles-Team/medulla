"""Served proof for Trino and Spark job-form submissions (R053.701).

A host opens an approved source's bounded read through ``ForeignReadOpen`` with a
``submission``. The server derives the idempotent submission id (SHA-256 of the
preset, input digests and parameters), refuses a missing fence field with
``FENCE_FIELD_MISSING``, an unknown snapshot with ``SNAPSHOT_UNKNOWN`` and an
unoffered preset with ``PRESET_UNAVAILABLE``, and answers with the fence fields.
A resubmission names the one execution it already started.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import sys
from pathlib import Path
from typing import Any

import pytest

_TESTS = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_TESTS))
sys.path.insert(0, str(_TESTS / "connector_competency"))

from conftest import TEST_AGENT_ID, TEST_TENANT, request_context  # noqa: E402
from test_connector_pack_client import _catalog  # noqa: E402
from test_connector_pack_planted import _context  # noqa: E402
from test_contract_kinds_served import (  # noqa: E402
    OPERATION,
    SOURCE_ID,
    _contract_iri,
    _contract_ttl,
    _spec,
)

from medulla.client import MedullaClient  # noqa: E402
from medulla.connector_pack import connector_pack_import_key  # noqa: E402
from medulla.generated.connector_pack import (  # noqa: E402
    ConnectorPackArchiveBuilder,
    ConnectorPackClient,
    ConnectorPackEntryContent,
    PackAnnotations,
    PackEntryKind,
    PackProducer,
)

KIND = "http_endpoint"
CONNECTOR = "ck-submission"
LIMITS = {
    "max_rows": 10,
    "max_bytes": 4096,
    "chunk_rows": 8,
    "chunk_bytes": 2048,
    "deadline_ms": 10_000,
}


async def _approved_receipt(client: MedullaClient) -> dict[str, Any]:
    """Bind, import one access-contract pack and approve its source."""
    importer = "principal:sha256:" + hashlib.sha256(TEST_AGENT_ID.encode()).hexdigest()
    bind_key = f"r053701:{CONNECTOR}:bind"
    await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "bind",
                "request": {
                    "context": _context(bind_key),
                    "connector": CONNECTOR,
                    "importer": importer,
                },
            }
        },
        None,
        idempotency_key=bind_key,
    )
    assert (
        await client.query.register_foreign_source(SOURCE_ID, _spec(CONNECTOR, KIND))
        == SOURCE_ID
    )
    contract_bytes = _contract_ttl(CONNECTOR, KIND).encode()
    resource_uri = f"contract://{CONNECTOR}"
    entries = [
        ConnectorPackEntryContent(
            kind=PackEntryKind.TOOL,
            uri=f"tool://{CONNECTOR}/{OPERATION}",
            name=OPERATION,
            media_type="application/json",
            body=b"{}",
            input_schema=b'{"type":"object"}',
            output_schema=b'{"type":"object"}',
            annotations=PackAnnotations(contract_version="1.0.0"),
        ),
        ConnectorPackEntryContent(
            kind=PackEntryKind.RESOURCE,
            uri=resource_uri,
            name="source-contract",
            media_type="text/turtle",
            body=contract_bytes,
            input_schema=b'{"type":"object"}',
            output_schema=b'{"type":"object"}',
            annotations=PackAnnotations(contract_version="1.0.0"),
        ),
    ]
    server_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.MCP_SERVER,
        uri=f"mcp-server://{CONNECTOR}",
        name=CONNECTOR,
        media_type="application/json",
        body=json.dumps({"name": CONNECTOR}).encode(),
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    archive = ConnectorPackArchiveBuilder.build(server_entry, entries)
    blob = await ConnectorPackClient(client).upload_archive(
        archive, operation_key=f"r053701:{CONNECTOR}:archive"
    )
    index = archive.index(
        connector=CONNECTOR,
        blob_digest=blob,
        server_package_version="1.0.0",
        producer=PackProducer(name="r053701-submission", version="1"),
        catalog=_catalog(),
    )
    key = connector_pack_import_key(CONNECTOR, index.pack_digest, None)
    imported = await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "import",
                "request": {
                    "context": _context(key),
                    "index": index.model_dump(mode="json"),
                    "expected_head": None,
                    "allow_mass_withdrawal": False,
                },
            }
        },
        None,
        idempotency_key=key,
    )
    assert imported["result"] == "imported", imported
    receipt = imported["receipt"]
    head = {
        "binding_revision": receipt["binding_revision"],
        "pack_digest": receipt["pack_digest"],
    }
    approve_key = f"r053701:{CONNECTOR}:approve"
    approved = await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "approve_source",
                "request": {
                    "context": _context(approve_key),
                    "connector": CONNECTOR,
                    "contract_iri": _contract_iri(CONNECTOR),
                    "resource_uri": resource_uri,
                    "expected_head": head,
                    "contract_digest": hashlib.sha256(contract_bytes).hexdigest(),
                },
            }
        },
        None,
        idempotency_key=approve_key,
    )
    assert approved["registration"]["approval"] == "approved", approved
    return approved


def _submission(approved: dict[str, Any], **override: Any) -> dict[str, Any]:
    submission = {
        "preset_id": OPERATION,
        "input_digests": ["sha256:aa", "sha256:bb"],
        "parameters": {"window": "7d"},
        "snapshot_id": approved["head"]["pack_digest"],
        "producer_revision": "r1",
    }
    submission.update(override)
    return submission


async def _exercise() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    other = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context={
            **request_context(agent_id="other-tenant-agent"),
            "tenant": TEST_TENANT + "-other",
        },
    )
    try:
        approved = await _approved_receipt(client)
        text = f"FOREIGN SCAN '{SOURCE_ID}'"
        prepared = await client.query.uql(text, prepare=True)
        pins = {
            "plan_digest": prepared["plan_digest"],
            "query_digest": prepared["query_digest"],
        }

        def request(submission: dict[str, Any] | None) -> dict[str, Any]:
            return {
                "request": {
                    "schema_version": 1,
                    "source_name": SOURCE_ID,
                    "approval": approved,
                    "limits": LIMITS,
                    "submission": submission,
                    **pins,
                }
            }

        # Two identical submissions name one run: one id, one execution.
        first = await client._send(
            "ForeignReadOpen", request(_submission(approved)), None
        )
        again = await client._send(
            "ForeignReadOpen", request(_submission(approved)), None
        )
        evidence = first["submission"]
        assert len(evidence["submission_id"]) == 64, first
        assert all(c in "0123456789abcdef" for c in evidence["submission_id"]), first
        assert again["submission"] == evidence, (first, again)
        assert again["execution"] == first["execution"], (first, again)
        assert evidence["source_id"] == SOURCE_ID, evidence
        assert evidence["snapshot_id"] == approved["head"]["pack_digest"], evidence
        assert evidence["generation"] == approved["head"]["binding_revision"], evidence
        assert evidence["producer_revision"] == "r1", evidence

        # A different input is a different run.
        changed = await client._send(
            "ForeignReadOpen",
            request(_submission(approved, input_digests=["sha256:cc"])),
            None,
        )
        assert changed["submission"]["submission_id"] != evidence["submission_id"]
        assert changed["execution"] != first["execution"]

        # Typed refusals, nothing opened.
        for override, code in (
            ({"producer_revision": ""}, "FENCE_FIELD_MISSING"),
            ({"snapshot_id": "sha256:not-a-registered-snapshot"}, "SNAPSHOT_UNKNOWN"),
            ({"preset_id": "hourly"}, "PRESET_UNAVAILABLE"),
        ):
            with pytest.raises(Exception) as refusal:
                await client._send(
                    "ForeignReadOpen", request(_submission(approved, **override)), None
                )
            assert code in str(refusal.value), (code, str(refusal.value))

        # Cross-tenant negative: another tenant's client never resumes, sees or
        # learns this tenant's submission; the same request is refused.
        with pytest.raises(Exception) as cross:
            await other._send("ForeignReadOpen", request(_submission(approved)), None)
        assert evidence["submission_id"] not in str(cross.value), str(cross.value)
        still = await client._send(
            "ForeignReadOpen", request(_submission(approved)), None
        )
        assert still["execution"] == first["execution"]
    finally:
        await other.close()
        await client.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R053.701")
def test_resubmission_returns_same_submission(clean_graph) -> None:
    asyncio.run(_exercise())
