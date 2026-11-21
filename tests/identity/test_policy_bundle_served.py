"""Served policy rule bundle (MEDULLA-IDENTITY-R702).

A bundle rides the served ``Identity`` method as the ``policy`` op family. Bound to a graph,
it gives the members of its group write access through the one access boundary, a second
bundle on the bound scope is refused, and unbinding removes the access.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient

ADMIN_SCOPES = [
    "identity:admin",
    "identity:read",
    "identity:authenticate",
    "security:check",
]


def _identity(client: SyncMedullaClient, op: dict) -> object:
    return client._send(
        "Identity", {"op": op}, None, idempotency_key=f"policy-{uuid.uuid4().hex}"
    )


def _bundle(bundle_id: str, group: str, graph: str) -> dict:
    return {
        "schema_version": 1,
        "bundle_id": bundle_id,
        "scope": {"graph": {"graph": graph}},
        "rules": [
            {
                "principal_group": group,
                "action": "Write",
                "resource_scope": {"Graph": graph},
                "effect": "Allow",
            }
        ],
    }


@pytest.mark.spec("MEDULLA-IDENTITY-R702")
def test_bundle_binds_and_enforces() -> None:
    tag = uuid.uuid4().hex[:8]
    graph = f"policy-{tag}"
    group = f"editors-{tag}"
    member = f"usr:policy-member-{tag}"
    outsider = f"usr:policy-outsider-{tag}"
    system = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(scopes=["*"]),
    )
    admin = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(scopes=ADMIN_SCOPES),
    )
    try:
        system.tenants.create(graph)
        try:
            _identity(
                admin,
                {
                    "family": "config",
                    "op": "initialize",
                    "request": {
                        "mode": "local",
                        "admin_username": "policy-root",
                        "admin_password": "correct horse battery staple",
                    },
                },
            )
        except EngineResponseError as refused:
            # Another served test may have initialized the store first.
            assert "IDENTITY_ALREADY_INITIALIZED" in str(refused)
        _identity(
            admin,
            {
                "family": "access",
                "op": "upsert_group",
                "request": {"group_id": group, "name": group},
            },
        )
        for username, principal, groups in (
            (f"policy-member-{tag}", member, [group]),
            (f"policy-outsider-{tag}", outsider, []),
        ):
            _identity(
                admin,
                {
                    "family": "user",
                    "op": "create",
                    "request": {
                        "username": username,
                        "kind": "human",
                        "principal_id": principal,
                        "groups": groups,
                    },
                },
            )
        assert admin.consensus.check_access(member, "write") is False

        _identity(
            admin,
            {"family": "policy", "op": "validate", "bundle": _bundle("b1", group, graph)},
        )
        assert admin.consensus.check_access(member, "write") is False, "validate binds nothing"

        _identity(
            admin,
            {
                "family": "policy",
                "op": "bind",
                "bundle": _bundle(f"b1-{tag}", group, graph),
            },
        )
        assert admin.consensus.check_access(member, "write") is True
        assert admin.consensus.check_access(outsider, "write") is False

        with pytest.raises(EngineResponseError) as bound:
            _identity(
                admin,
                {
                    "family": "policy",
                    "op": "bind",
                    "bundle": _bundle(f"b2-{tag}", group, graph),
                },
            )
        assert "POLICY_SCOPE_BOUND" in str(bound.value)

        _identity(
            admin,
            {
                "family": "policy",
                "op": "unbind",
                "scope": {"graph": {"graph": graph}},
            },
        )
        assert admin.consensus.check_access(member, "write") is False
    finally:
        try:
            system.tenants.delete(graph)
        finally:
            admin.close()
            system.close()
