"""Pure wire-shape tests for the native WorkItem capability client surface."""

from __future__ import annotations

from typing import Any

import pytest

from medulla.client import (
    MedullaClient,
    WorkItemClient,
    _work_item_capability_result,
)

pytestmark = pytest.mark.no_engine


class _FakeGraphClient(MedullaClient):
    def __init__(self, result: dict[str, object]) -> None:
        self.result = result
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def _send(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        graph: str | None = None,
        *,
        idempotency_key: str | None = None,
    ) -> Any:
        del graph, idempotency_key
        assert params is not None
        self.calls.append((method, params))
        return self.result


@pytest.mark.asyncio
async def test_mint_and_verify_bind_only_opaque_wire_fields() -> None:
    mint_transport = _FakeGraphClient(
        {
            "schema_version": "1",
            "decision": "minted",
            "valid": True,
            "capability": b"WIC1" + b"n" * 32,
        }
    )
    client = WorkItemClient(mint_transport)
    minted = await client.mint_capability(
        {"schema_version": "1", "work_item_id": "work-1"}
    )
    assert minted["decision"] == "minted"
    assert mint_transport.calls == [
        (
            "MintWorkItemClaimCapability",
            {"request": {"schema_version": "1", "work_item_id": "work-1"}},
        )
    ]
    assert "tenant" not in mint_transport.calls[0][1]["request"]
    assert "lease_epoch" not in mint_transport.calls[0][1]["request"]

    verify_transport = _FakeGraphClient(
        {
            "schema_version": "1",
            "decision": "unauthorized",
            "valid": False,
            "capability": None,
        }
    )
    verifier = WorkItemClient(verify_transport)
    denied = await verifier.verify_capability(
        {
            "schema_version": "1",
            "work_item_id": "work-1",
            "capability": b"WIC1" + b"n" * 32,
        }
    )
    assert denied == {
        "schema_version": "1",
        "decision": "unauthorized",
        "valid": False,
        "capability": None,
    }


def test_capability_result_rejects_unknown_fields_and_authority_projection() -> None:
    with pytest.raises(ValueError, match="unsupported fields"):
        _work_item_capability_result(
            {
                "schema_version": "1",
                "decision": "verified",
                "valid": True,
                "capability": None,
                "lease_epoch": 1,
            },
            verify=True,
        )
    with pytest.raises(ValueError, match="must not return capability"):
        _work_item_capability_result(
            {
                "schema_version": "1",
                "decision": "verified",
                "valid": True,
                "capability": b"WIC1" + b"n" * 32,
            },
            verify=True,
        )


@pytest.mark.asyncio
async def test_capability_request_does_not_accept_public_authority_tuple() -> None:
    client = WorkItemClient(_FakeGraphClient({}))
    with pytest.raises(ValueError, match="unsupported fields"):
        await client.mint_capability(
            {
                "schema_version": "1",
                "work_item_id": "work-1",
                "lease_epoch": 1,
            }
        )


# spec: MEDULLA-DURABLE-KERNEL-R030.1.1
# spec: MEDULLA-DURABLE-KERNEL-R030.1.4
@pytest.mark.asyncio
async def test_host_start_grant_round_trip() -> None:
    from medulla.client import ControlLeaseClient
    from medulla.generated.models import (
        ControlLeaseIssued,
        HostStartBinding,
        IssueHostStartGrantRequest,
    )

    binding = {
        "principal_ref": "beneficiary",
        "session_ref": "opaque-session",
        "workspace_ref": "workspace",
        "configuration_digest": "a" * 64,
        "interfaces": "terminal",
        "web_logs": False,
        "policy_revision": "policy-1",
        "session_expires_at_ms": 61_000,
    }
    lease = {
        "lease_id": "host-grant",
        "kind": "host.start",
        "status": "active",
        "grant": binding,
        "issued_at_ms": 1_000,
        "expires_at_ms": 61_000,
        "hard_expires_at_ms": 61_000,
        "revision": 1,
        "host_start_provenance": {
            "schema_version": 1,
            "issuer_ref": "principal:sha256:" + "b" * 64,
        },
    }
    wire: dict[str, object] = {
        "outcome": "issued",
        "lease": lease,
        "changed_work_item_ids": [],
    }
    ControlLeaseIssued.model_validate(wire)
    transport = _FakeGraphClient(wire)
    answer = await ControlLeaseClient(transport).issue_host_start(
        tenant="tenant-shared",
        lease_id="host-grant",
        binding=binding,
        expires_at_ms=61_000,
        idempotency_key="host-issue",
    )
    assert transport.calls[0][0] == "IssueHostStartGrant"
    sent = transport.calls[0][1]["request"]
    IssueHostStartGrantRequest.model_validate(sent)
    assert answer["outcome"] == "issued"
    assert answer["lease"] == lease
    assert (
        HostStartBinding.model_validate(answer["lease"]["grant"]).model_dump(
            mode="json"
        )
        == binding
    )
