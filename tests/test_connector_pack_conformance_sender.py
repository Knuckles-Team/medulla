"""MEDULLA-TYPED-PACKS-R807: the typed ``conformance_evidence`` sender (L1).

The sender builds the one ``ConnectorPack`` method with the ``conformance_evidence``
op and decodes the engine's record into the generated ``ConformanceEvidence``.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from medulla.connector_pack import ConnectorPackClient
from medulla.generated.connector_pack import ConformanceEvidenceState

DIGEST = "sha256:" + "ab" * 32


class _Transport:
    def __init__(self, payload: dict[str, Any]) -> None:
        self.payload = payload
        self.calls: list[tuple[str, dict[str, Any], str | None]] = []

    async def _send(
        self,
        method: str,
        params: dict[str, Any],
        graph: str | None,
        *,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        self.calls.append((method, params, graph))
        return self.payload


# spec: MEDULLA-TYPED-PACKS-R807
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R807")
def test_conformance_evidence_sender_builds_the_op_and_decodes_the_record() -> None:
    transport = _Transport(
        {
            "connector": "c1",
            "expires_at": 2_000,
            "issued_at": 1_000,
            "pack_digest": DIGEST,
            "revision": 3,
            "state": "active",
            "tenant_id": "t1",
            "tool_fingerprints": {"tool://c1/search": DIGEST},
        }
    )
    evidence = asyncio.run(
        ConnectorPackClient(transport).conformance_evidence(  # type: ignore[arg-type]
            tenant_id="t1", connector="c1"
        )
    )
    assert transport.calls == [
        (
            "ConnectorPack",
            {
                "op": {
                    "op": "conformance_evidence",
                    "request": {"tenant_id": "t1", "connector": "c1"},
                }
            },
            None,
        )
    ]
    assert evidence.state is ConformanceEvidenceState.ACTIVE
    assert evidence.revoked_at is None
    assert evidence.revision == 3
    assert evidence.tool_fingerprints == {"tool://c1/search": DIGEST}
