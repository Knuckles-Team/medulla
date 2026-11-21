"""Governed-ledger served harness (MEDULLA-SRC-AUTH-R003.6).

Every served test of the source-authority spec imports this module. It uses only served
methods that exist on main: a fresh graph with three clients on it (the System admin, a
proposer and an approver), the governed-change propose and approve operations, the
approval-lease issue and consume operations, ``GraphSchemaOp::AttachApproved`` and the
``SourceCoverage`` read-back.
"""

from __future__ import annotations

import hashlib
import json
import os
import time
import uuid
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any

import msgpack
from conftest import TEST_TENANT, register_commons_agents, request_context

from medulla.client import MedullaClient, SyncMedullaClient
from medulla.generated import coordination, reasoning, security

# The governed kind of a source-authority change (`SOURCE_AUTHORITY_KIND`,
# crates/medulla-mutation-types/src/governed_change.rs).
SOURCE_AUTHORITY_KIND = "governed.source-authority"

# The exact governance scopes the proposer and the approver hold (the governed ledger reads
# only exact `governance:*` scopes; `*` grants none). Both hold both, so the self-approval
# refusal is the ledger's two-person rule rather than a missing scope.
GOVERNANCE_SCOPES = ["governance:propose", "governance:approve-source-authority"]

SOURCE_ID = "approved:drift-served"
APPROVAL_LEASE_KIND = "action.approval"
APPROVAL_ACTION = "schema_repair"
APPROVED_CANDIDATE_DOMAIN = "eg/approved-schema-candidate/v1"
LEASE_SPAN_MS = 600_000
PROPOSAL_SPAN_MS = 600_000

FieldSpec = dict[str, object]
Fields = dict[str, FieldSpec]


@dataclass
class GovernedLedger:
    """A fresh graph and the three clients that act on it."""

    graph: str
    admin: MedullaClient
    proposer: MedullaClient
    approver: MedullaClient
    # The candidate digest each proposed change was made over, by change id.
    digests: dict[str, str] = field(default_factory=dict)


async def _connect(graph: str | None = None, **claims: Any) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(**claims),
        **({"graph_name": graph} if graph else {}),
    )


@asynccontextmanager
async def governed_graph() -> AsyncIterator[GovernedLedger]:
    """A fresh graph ``srcauth-<8 hex>``, two distinct registered agents, three clients."""
    tag = uuid.uuid4().hex[:8]
    graph = f"srcauth-{tag}"
    proposer_id = f"srcauth-proposer-{tag}"
    approver_id = f"srcauth-approver-{tag}"
    system = await _connect()
    clients: list[MedullaClient] = []
    try:
        await system.tenants.create(graph)
        register_commons_agents(proposer_id, approver_id)
        # A registered agent reads and writes the commons only; the ledger graph needs its
        # own grant for the two agents to act on it.
        registry = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
        try:
            for action in ("Read", "Write"):
                registry.rbac.add_grant("commons-access", {"Graph": graph}, action)
        finally:
            registry.close()
        admin = await _connect(graph)
        clients.append(admin)
        proposer = await _connect(
            graph, agent_id=proposer_id, scopes=["*", *GOVERNANCE_SCOPES]
        )
        clients.append(proposer)
        approver = await _connect(
            graph, agent_id=approver_id, scopes=["*", *GOVERNANCE_SCOPES]
        )
        clients.append(approver)
        yield GovernedLedger(graph, admin, proposer, approver)
    finally:
        try:
            await system.tenants.delete(graph)
        finally:
            for client in clients:
                await client.close()
            await system.close()


# --- the candidate and its digest ---------------------------------------------------


def canonical_contract(contract: dict[str, Any]) -> str:
    """The contract's canonical JSON: the sorted-key encoding the Rust side
    takes the approval digest over (BTreeMap field order, declaration-order
    field members)."""
    body: dict[str, Any] = {"fields": contract["fields"]}
    if contract.get("source_authority"):
        # Omitted when false, so an ordinary candidate keeps its digest.
        body["source_authority"] = True
    return json.dumps(body, sort_keys=True, separators=(",", ":"))


def candidate_digest(contract: dict[str, Any], source_id: str = SOURCE_ID) -> str:
    """The digest an approval grant must bind: domain, key and canonical
    contract, each NUL-terminated (``approved_candidate_digest``)."""
    framed = (
        "\0".join([APPROVED_CANDIDATE_DOMAIN, source_id, canonical_contract(contract)])
        + "\0"
    )
    return hashlib.sha256(framed.encode("utf-8")).hexdigest()


# One approved record contract (two required fields) for harness tests that need a known
# contract to attach.
BASELINE_FIELDS_CONTRACT: dict[str, Fields] = {
    "fields": {
        "id": {"required": True, "types": ["integer"]},
        "name": {"required": True, "types": ["string"]},
    }
}


# --- the governed change -------------------------------------------------------------


async def propose_authority(
    ledger: GovernedLedger, source_id: str, contract: dict[str, Any]
) -> str:
    """Propose a source-authority change as the proposer; returns its change id."""
    change_id = f"srcauth-change-{uuid.uuid4().hex[:8]}"
    digest = candidate_digest(contract, source_id)
    await security.send_governed_change(
        ledger.proposer,
        {
            "op": {
                "op": "propose",
                "request": {
                    "change_id": change_id,
                    "kind": SOURCE_AUTHORITY_KIND,
                    "target": source_id,
                    "digest": digest,
                    "justification": "source authority harness",
                    "proposal_ttl_ms": PROPOSAL_SPAN_MS,
                    "window_ms": LEASE_SPAN_MS,
                },
            }
        },
        idempotency_key=f"propose:{change_id}",
    )
    ledger.digests[change_id] = digest
    return change_id


async def approve_authority(
    ledger: GovernedLedger, change_id: str, as_client: MedullaClient
) -> None:
    """Approve a proposed change as ``as_client`` (the self-approval test passes the proposer)."""
    await security.send_governed_change(
        as_client,
        {
            "op": {
                "op": "approve",
                "request": {
                    "change_id": change_id,
                    "digest": ledger.digests[change_id],
                },
            }
        },
        idempotency_key=f"approve:{change_id}:{uuid.uuid4().hex[:8]}",
    )


# --- approval lease and attach -------------------------------------------------------


def approval_lease_id(change_id: str) -> str:
    """The approval reference of ``change_id`` (``governed_approval_ref`` in Rust): the change, not a lease."""
    return f"governed-change:{change_id}"

async def issue_approval_lease(
    client: MedullaClient,
    lease_id: str,
    candidate: str,
    source_id: str = SOURCE_ID,
    action: str = APPROVAL_ACTION,
) -> None:
    now_ms = int(time.time() * 1000)
    await coordination.send_issue_control_lease(
        client,
        {
            "request": {
                "tenant": TEST_TENANT,
                "lease_id": lease_id,
                "kind": APPROVAL_LEASE_KIND,
                "grant": {
                    "kind": action,
                    "target": source_id,
                    "candidate_digest": candidate,
                },
                "issued_at_ms": now_ms,
                "expires_at_ms": now_ms + LEASE_SPAN_MS,
                "hard_expires_at_ms": now_ms + LEASE_SPAN_MS,
                "idempotency_key": f"issue:{lease_id}",
            }
        },
    )


async def consume_approval_lease(client: MedullaClient, lease_id: str) -> None:
    await coordination.send_transition_control_lease(
        client,
        {
            "request": {
                "tenant": TEST_TENANT,
                "lease_id": lease_id,
                "expected_revision": 1,
                "to": "consumed",
                "idempotency_key": f"consume:{lease_id}",
            }
        },
    )


def _contract(fields: Fields, source_authority: bool) -> dict[str, Any]:
    """The wire contract; `source_authority` is sent only when set, as the Rust side omits it."""
    contract: dict[str, Any] = {"fields": fields}
    if source_authority:
        contract["source_authority"] = True
    return contract


async def attach_approved(
    client: MedullaClient,
    lease_id: str,
    fields: dict[str, dict[str, object]],
    source_id: str = SOURCE_ID,
    expected_digest: str | None = None,
    source_authority: bool = False,
    idempotency_key: str | None = None,
) -> Any:
    """Attach under ``lease_id``; a repeated ``idempotency_key`` is a retry of the same request."""
    return await reasoning.send_graph_schema(
        client,
        {
            "op": {
                "op": "attach_approved",
                "source_id": source_id,
                "contract": _contract(fields, source_authority),
                "approval_lease_id": lease_id,
                "expected_digest": expected_digest,
                "if_composed_digest": None,
            }
        },
        idempotency_key=idempotency_key or f"attach:{lease_id}:{uuid.uuid4().hex}",
    )


async def validate_repair(
    client: MedullaClient,
    fields: dict[str, dict[str, object]],
    source_id: str = SOURCE_ID,
    source_authority: bool = False,
) -> bool:
    """Run the shadow-graph validation; returns the op's `changed` flag."""
    committed = await reasoning.send_graph_schema(
        client,
        {
            "op": {
                "op": "validate_repair",
                "source_id": source_id,
                "contract": _contract(fields, source_authority),
                "if_composed_digest": None,
            }
        },
        idempotency_key=f"validate:{uuid.uuid4().hex}",
    )
    return committed.changed


# --- read-back -----------------------------------------------------------------------


async def authority_rows(client: MedullaClient, classes: list[str]) -> list[dict]:
    """The `verdicts` of served ``SourceCoverage`` for ``classes`` (hop budget zero)."""
    result = await client._send(
        "SourceCoverage", {"classes": classes, "hop_budget": 0}, None
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result["verdicts"]


async def attach_operator_authority(
    ledger: GovernedLedger, source_id: str, ontology_ttl: str
) -> None:
    """Attach a Turtle document of ``vg:SourceAuthority`` rows under `Operator` origin, as admin."""
    await reasoning.send_graph_schema(
        ledger.admin,
        {
            "op": {
                "op": "attach",
                "source_id": source_id,
                "ontology_ttl": ontology_ttl,
            }
        },
        idempotency_key=f"attach-operator:{uuid.uuid4().hex}",
    )
