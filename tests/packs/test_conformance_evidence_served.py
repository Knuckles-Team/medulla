"""MEDULLA-TYPED-PACKS-R807 served acceptance: connector conformance evidence.

The connector onboarding import writes one ``ConformanceEvidence`` record with the
pack head. The served ``ConnectorPack`` read returns it (pack digest, one fingerprint
per tool, issue and expiry times); the served revoke ends it; a revoked record, and
any other tenant's read, refuse ``CONFORMANCE_EVIDENCE_ABSENT``.
"""

from __future__ import annotations

import json
import os
import sys
import time
import uuid
from pathlib import Path

import pytest
from conftest import TEST_TENANT, request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries
from test_connector_pack_planted import _context, _pack_op

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.connector_pack import (
    ConformanceEvidence,
    ConformanceEvidenceState,
    PackAnnotations,
    PackEntryKind,
)

OTHER_TENANT = "tenant:evidence-other"
ABSENT = "CONFORMANCE_EVIDENCE_ABSENT"


def _tool(connector: str, name: str) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{name}",
        name=name,
        media_type="application/json",
        body=json.dumps({"name": name, "description": "A tool."}).encode(),
        input_schema=b'{"type":"object"}',
        annotations=PackAnnotations(),
    )


async def _read(
    client: MedullaClient, tenant: str, connector: str
) -> ConformanceEvidence:
    raw = await _pack_op(
        client,
        "conformance_evidence",
        {"tenant_id": tenant, "connector": connector},
        f"evidence-read:{connector}:{uuid.uuid4().hex}",
    )
    return ConformanceEvidence.model_validate(raw)


async def _revoke(client: MedullaClient, connector: str) -> ConformanceEvidence:
    key = f"evidence-revoke:{connector}"
    raw = await _pack_op(
        client,
        "revoke_conformance_evidence",
        {"context": _context(key), "connector": connector},
        key,
    )
    return ConformanceEvidence.model_validate(raw)


@pytest.mark.asyncio
async def test_evidence_written_read_revoked_and_tenant_scoped() -> None:
    client = await connect()
    other_context = request_context()
    other_context["tenant"] = OTHER_TENANT
    outsider = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=other_context,
    )
    try:
        connector = f"evidence-{uuid.uuid4().hex[:8]}"

        # Before any import there is no evidence.
        with pytest.raises(EngineResponseError, match=ABSENT):
            await _read(client, TEST_TENANT, connector)

        # The onboarding import writes the evidence with the pack head.
        before_ms = int(time.time() * 1000)
        imported = await import_entries(
            client,
            connector,
            [_tool(connector, "search"), _tool(connector, "fetch")],
        )
        assert imported["result"] == "imported", imported
        evidence = await _read(client, TEST_TENANT, connector)
        assert evidence.tenant_id == TEST_TENANT
        assert evidence.connector == connector
        assert evidence.state == ConformanceEvidenceState.ACTIVE
        assert evidence.revoked_at is None
        assert evidence.revision == imported["receipt"]["binding_revision"]
        assert evidence.pack_digest == imported["receipt"]["pack_digest"]
        assert set(evidence.tool_fingerprints) == {
            f"tool://{connector}/search",
            f"tool://{connector}/fetch",
        }
        assert evidence.expires_at > evidence.issued_at >= before_ms - 60_000

        # Another tenant never reaches this tenant's record.
        with pytest.raises(EngineResponseError, match=ABSENT):
            await _read(outsider, OTHER_TENANT, connector)

        # Reading under another tenant's name is an access refusal, not a lookup.
        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            await _read(client, OTHER_TENANT, connector)

        # Revoking ends the evidence: the record keeps its data and gains the
        # revocation time, and every later read refuses.
        revoked = await _revoke(client, connector)
        assert revoked.state == ConformanceEvidenceState.REVOKED
        assert revoked.revoked_at is not None
        assert revoked.pack_digest == evidence.pack_digest
        assert revoked.tool_fingerprints == evidence.tool_fingerprints
        with pytest.raises(EngineResponseError, match=ABSENT):
            await _read(client, TEST_TENANT, connector)

        # A second revoke of the same record refuses rather than rewriting the time.
        with pytest.raises(EngineResponseError, match=ABSENT):
            await _pack_op(
                client,
                "revoke_conformance_evidence",
                {"context": _context("evidence-revoke-again"), "connector": connector},
                f"evidence-revoke-again:{connector}",
            )
    finally:
        await outsider.close()
        await client.close()
