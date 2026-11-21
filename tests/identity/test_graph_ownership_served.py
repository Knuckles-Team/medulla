"""Served graph ownership plan, apply and rollback (MEDULLA-IDENTITY-R700.3 to R700.6).

A graph's creator and per-principal write counts come from the durable write ledger
(R700.7). `GraphOwnershipPlan` turns them into a deterministic plan, `RbacAdmin`
`ApplyOwnership` replaces each planned principal's role set in one transaction and stores
a rollback record, and `RollbackOwnership` restores the prior sets exactly.
"""

import re
import uuid

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import (
        TEST_AGENT_ID,
        TEST_SIGNER_KEY,
        bootstrap_context,
        request_context,
    )
    from identity.test_share_restart_served import AUTH_SECRET, _engine
except ModuleNotFoundError:  # package-root import
    from tests.conftest import (
        TEST_AGENT_ID,
        TEST_SIGNER_KEY,
        bootstrap_context,
        request_context,
    )
    from tests.identity.test_share_restart_served import AUTH_SECRET, _engine

P1 = "usr:own-creator"
P2 = "usr:own-writer"
P3 = "usr:own-reader"
SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")
PRIOR = {P1: ["commons-access"], P2: ["commons-access", "extra"], P3: []}


def _connect(socket_path: str, principal: str, graph: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=AUTH_SECRET,
        graph_name=graph,
        verified_context=request_context(agent_id=principal),
    )


def _system(socket_path: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=AUTH_SECRET,
        verified_context=request_context(),
    )


def _register(system: SyncMedullaClient, principal: str, roles: list[str]) -> None:
    system.consensus.register_identity(
        principal,
        "Agent",
        [],
        roles,
        signer_id=TEST_AGENT_ID,
        signer_key=TEST_SIGNER_KEY,
    )


def _roles(system: SyncMedullaClient, principal: str) -> set[str]:
    identity = system.consensus.get_identity(principal)
    assert identity is not None
    return set(identity["roles"])


def _bootstrap(socket_path: str) -> None:
    bootstrap = SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=AUTH_SECRET,
        verified_context=bootstrap_context(),
    )
    try:
        bootstrap.consensus.bootstrap_system_identity(
            agent_id=TEST_AGENT_ID,
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
    finally:
        bootstrap.close()


def _fixture(socket_path: str) -> str:
    """P1 creates graph `own-<8 hex>`, P2 writes 20 nodes and P3 writes 1; each principal
    then holds its prior role set. Returns the graph name."""
    graph = f"own-{uuid.uuid4().hex[:8]}"
    access = f"{graph}-access"
    system = _system(socket_path)
    try:
        system.rbac.add_role("commons-access")
        system.rbac.add_role(access)
        for action in ("Read", "Write"):
            system.rbac.add_grant(access, {"Graph": graph}, action)
        for principal in (P1, P2, P3):
            _register(system, principal, ["commons-access", "extra", access])
        creator = _connect(socket_path, P1, graph)
        try:
            creator.tenants.create(graph, "Agent")
        finally:
            creator.close()
        for principal, count in ((P2, 20), (P3, 1)):
            writer = _connect(socket_path, principal, graph)
            try:
                for index in range(count):
                    writer.nodes.add(
                        f"{principal}-{index}", {"node_type": "Note", "_owner_id": principal}
                    )
            finally:
                writer.close()
        for principal, roles in PRIOR.items():
            _register(system, principal, roles)
    finally:
        system.close()
    return graph


def _plan(system: SyncMedullaClient, graph: str) -> dict:
    reply = system.rbac.ownership_plan(graph)
    assert SHA256_HEX.match(reply["plan_digest"])
    assert SHA256_HEX.match(reply["signals_digest"])
    return reply


def _apply(system: SyncMedullaClient, graph: str, reply: dict) -> dict:
    return system.rbac.ownership_apply(
        graph, reply["plan"], reply["plan_digest"], reply["signals_digest"]
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R700.3")
def test_plan_and_apply_reach_wire_methods(tmp_path):
    with _engine(tmp_path) as socket_path:
        _bootstrap(socket_path)
        graph = _fixture(socket_path)
        system = _system(socket_path)
        try:
            reply = _plan(system, graph)
            assert reply["plan"]
            applied = _apply(system, graph, reply)
            assert applied["principals"] == len(reply["plan"])
            assert applied["plan_digest"] == reply["plan_digest"]
        finally:
            system.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R700.4")
def test_unsafe_plans_are_refused(tmp_path):
    with _engine(tmp_path) as socket_path:
        _bootstrap(socket_path)
        graph = _fixture(socket_path)
        system = _system(socket_path)
        try:
            before = {principal: _roles(system, principal) for principal in PRIOR}
            reply = _plan(system, graph)
            widening = [dict(entry) for entry in reply["plan"]]
            widening[0] = {**widening[0], "full_role_set": ["graph:other:owner"]}
            with pytest.raises(EngineResponseError, match="OWNERSHIP_WIDENS_TENANT"):
                system.rbac.ownership_apply(
                    graph, widening, _digest_of(system, graph, widening), reply["signals_digest"]
                )
            with pytest.raises(EngineResponseError, match="OWNERSHIP_NO_ROLLBACK"):
                system.rbac.ownership_rollback(graph, "f" * 64)
            assert {principal: _roles(system, principal) for principal in PRIOR} == before
        finally:
            system.close()


def _digest_of(system: SyncMedullaClient, graph: str, plan: list[dict]) -> str:
    """The plan digest the engine expects: SHA-256 over the plan entries sorted by
    principal, serialized as compact JSON with the engine's field order."""
    import hashlib
    import json

    ordered = sorted(plan, key=lambda entry: entry["principal"])
    canonical = [
        {
            "schema_version": entry["schema_version"],
            "principal": entry["principal"],
            "disposition": entry["disposition"],
            "full_role_set": sorted(entry["full_role_set"]),
        }
        for entry in ordered
    ]
    return hashlib.sha256(
        json.dumps(canonical, separators=(",", ":")).encode()
    ).hexdigest()


@pytest.mark.spec("MEDULLA-IDENTITY-R700.5")
def test_stored_record_reproduces_plan(tmp_path):
    with _engine(tmp_path) as socket_path:
        _bootstrap(socket_path)
        graph = _fixture(socket_path)
        system = _system(socket_path)
        try:
            reply = _plan(system, graph)
            assert reply.get("applied") is None
            _apply(system, graph, reply)
            after = system.rbac.ownership_plan(graph)
            summary = after["applied"]
            assert summary["plan_digest"] == reply["plan_digest"]
            assert summary["rolled_back"] is False
            assert summary["actor"]
            assert _digest_of(system, graph, reply["plan"]) == summary["plan_digest"]
        finally:
            system.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R700.6")
def test_apply_then_rollback_restores_prior_roles(tmp_path):
    """The ledger makes the plan deterministic: P1 Owner (creator), P2 Writer (20 of 21
    writes), P3 Reader (1 of 21, below 0.10). The engine restarts between apply and
    rollback, so the rollback record is read from the persisted identity store."""
    with _engine(tmp_path) as socket_path:
        _bootstrap(socket_path)
        graph = _fixture(socket_path)
        system = _system(socket_path)
        try:
            reply = _plan(system, graph)
            dispositions = {entry["principal"]: entry["disposition"] for entry in reply["plan"]}
            assert dispositions == {P1: "owner", P2: "writer", P3: "reader"}
            _apply(system, graph, reply)
            assert _roles(system, P1) == {f"graph:{graph}:owner"}
            assert _roles(system, P2) == {f"graph:{graph}:writer"}
            assert _roles(system, P3) == {f"graph:{graph}:reader"}
        finally:
            system.close()
    with _engine(tmp_path) as socket_path:
        system = _system(socket_path)
        try:
            system.rbac.ownership_rollback(graph, reply["plan_digest"])
            for principal, roles in PRIOR.items():
                assert _roles(system, principal) == set(roles)
        finally:
            system.close()
