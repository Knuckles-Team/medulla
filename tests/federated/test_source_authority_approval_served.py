"""An authority candidate attaches only under its own approval (MEDULLA-SRC-AUTH-R003.3)."""

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "source_authority"))
from governed_ledger import (
    BASELINE_FIELDS_CONTRACT,
    approval_lease_id,
    approve_authority,
    attach_approved,
    governed_graph,
    propose_authority,
    validate_repair,
)

from medulla.client import EngineResponseError
from medulla.generated import reasoning
from medulla.generated.graph_schema import SchemaSourceOriginViewApproved


@pytest.mark.spec("MEDULLA-SRC-AUTH-R003.3")
@pytest.mark.asyncio
async def test_authority_attach_needs_distinct_approver() -> None:
    tag = uuid.uuid4().hex[:8]
    source_id = f"approved:srcauth-{tag}"
    fields = BASELINE_FIELDS_CONTRACT["fields"]
    authority = {"fields": fields, "source_authority": True}
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

        # The approved change is the approval: the attach names it and no
        # lease is issued.
        lease_id = approval_lease_id(change_id)

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

        # Digest X attaches X.
        await validate_repair(ledger.admin, fields, source_id, source_authority=True)
        first = await attach_approved(
            ledger.admin,
            lease_id,
            fields,
            source_id,
            source_authority=True,
            idempotency_key=f"attach-authority-{tag}",
        )
        listed = await reasoning.send_graph_schema_list(ledger.admin, {})
        attached = [s for s in listed.dynamic_sources if s.source_id == source_id]
        assert len(attached) == 1
        assert isinstance(attached[0].origin, SchemaSourceOriginViewApproved)
        assert attached[0].origin.approval_lease_id == lease_id

        # The approval is single use: the attach wrote its spent node in its own
        # commit, so a different second attach is refused.
        with pytest.raises(EngineResponseError) as refused:
            await attach_approved(
                ledger.admin, lease_id, fields, source_id, source_authority=True
            )
        assert refused.value.code == "SCHEMA_APPROVAL_REQUIRED"

        # A retry with the same request key replays the first receipt.
        replay = await attach_approved(
            ledger.admin,
            lease_id,
            fields,
            source_id,
            source_authority=True,
            idempotency_key=f"attach-authority-{tag}",
        )
        assert replay == first
