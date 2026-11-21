"""An authority candidate attaches only under its own approval (MEDULLA-SRC-AUTH-R003.3)."""

from __future__ import annotations

import os
import sys
import uuid
from pathlib import Path

import pytest
from conftest import tenant_request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "source_authority"))
from governed_ledger import (
    BASELINE_FIELDS_CONTRACT,
    SOURCE_AUTHORITY_KIND,
    approve_authority,
    attach_approved,
    candidate_digest,
    consume_approval_lease,
    governed_graph,
    issue_approval_lease,
    propose_authority,
    validate_repair,
)

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import reasoning
from medulla.generated.graph_schema import SchemaSourceOriginViewApproved


@pytest.mark.spec("MEDULLA-SRC-AUTH-R003.3")
@pytest.mark.asyncio
async def test_authority_attach_needs_distinct_approver() -> None:


    tag = uuid.uuid4().hex[:8]
    source_id = f"approved:srcauth-{tag}"
    lease_id = f"approval:srcauth-{tag}"
    fields = BASELINE_FIELDS_CONTRACT["fields"]
    authority = {"fields": fields, "source_authority": True}
    digest = candidate_digest(authority, source_id)
    async with governed_graph() as ledger:
        await ledger.admin.nodes.add(
            f"record-{tag}",
            {"type": f"urn:eg:source:srcauth-{tag}#Record", "id": 1, "name": "alpha"},
        )

        # The proposer cannot approve its own change; a distinct approver can.
        change_id = await propose_authority(ledger, source_id, authority)
        with pytest.raises(EngineResponseError) as refused:
            await approve_authority(ledger, change_id, ledger.proposer)
        assert refused.value.code == "GOVERNED_SELF_APPROVAL"
        await approve_authority(ledger, change_id, ledger.approver)

        # The approved digest is bound under the source-authority action.
        await issue_approval_lease(
            ledger.admin, lease_id, digest, source_id, action=SOURCE_AUTHORITY_KIND
        )
        await consume_approval_lease(ledger.admin, lease_id)

        # One changed byte changes the digest and fails the match.
        changed = {
            "id": {"required": True, "types": ["integer"]},
            "name": {"required": False, "types": ["string"]},
        }
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(
                ledger.admin, lease_id, changed, source_id, source_authority=True
            )
        assert refused.value.code == "SCHEMA_APPROVAL_MISMATCH"

        # A second tenant does not see the lease.
        foreign = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=tenant_request_context("tenant:foreign"),
            graph_name=ledger.graph,
        )
        try:
            with pytest.raises(EngineResponseError) as refused:
                await attach_approved(
                    foreign, lease_id, fields, source_id, source_authority=True
                )
            assert refused.value.code == "SCHEMA_APPROVAL_REQUIRED"
        finally:
            await foreign.close()

        # Digest X attaches X.
        await validate_repair(ledger.admin, fields, source_id, source_authority=True)
        await attach_approved(
            ledger.admin, lease_id, fields, source_id, source_authority=True
        )
        listed = await reasoning.send_graph_schema_list(ledger.admin, {})
        attached = [s for s in listed.dynamic_sources if s.source_id == source_id]
        assert len(attached) == 1
        assert isinstance(attached[0].origin, SchemaSourceOriginViewApproved)
        assert attached[0].origin.approval_lease_id == lease_id
