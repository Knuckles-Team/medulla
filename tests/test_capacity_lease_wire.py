"""MEDULLA-PLACEMENT-R786.7: `StartCapacity` and `RevokeCapacity` (build-slot lease).

One real native engine owns a build-slot cell. A lane acquires a slot, starts it, and an
administrator revokes it; the holder's next renew is the typed `TRANSITION_REFUSED`
that tells it to stop. The method catalog lists both methods with the scopes the
wire contract names.
"""

import hashlib
import json

import msgpack
import pytest

pytestmark = [
    pytest.mark.no_engine,  # The served case owns its real isolated process.
    pytest.mark.spec("MEDULLA-PLACEMENT-R786.7"),
]

CELL = "build-slots:r820"
TENANT = "tenant-a"
NOW_MS = 1_000_000
TTL_MS = 60_000
# The anonymous local principal's persistence id, as the engine derives it.
OWNER = "principal:sha256:" + hashlib.sha256(b"anonymous").hexdigest()


def _value(result):
    """A served result is a value, or the MessagePack of one."""
    if isinstance(result, (bytes, bytearray)):
        return msgpack.unpackb(result, raw=False)
    return result


def _acquire(rpc, ok, sock, key, priority):
    return _value(
        ok(
            rpc(
                sock,
                "AcquireCapacity",
                {
                    "request": {
                        "schema_version": "1",
                        "tenant_ref": TENANT,
                        "work_item_id": f"wi-{key}",
                        "owner_digest": OWNER,
                        "idempotency_key": key,
                        "priority": priority,
                        "demands": [
                            {
                                "cell_id": CELL,
                                "resource_class": "build_slot",
                                "amount": 1,
                            }
                        ],
                        "lease_id": None,
                        "ttl_ms": TTL_MS,
                        "now_ms": NOW_MS,
                        "cost_budget_micros": None,
                        "token_budget": None,
                        "queue_ttl_ms": None,
                    }
                },
                key=key,
            )
        )
    )


def _fence(lease):
    return {
        "lease_id": lease["lease_id"],
        "lease_epoch": lease["lease_epoch"],
        "fence_token": lease["fence_token"],
    }


def _mutate(rpc, sock, method, lease):
    return rpc(
        sock,
        method,
        {
            "request": {
                "schema_version": "1",
                "tenant_ref": TENANT,
                "owner_digest": OWNER,
                "leases": [_fence(lease)],
                "now_ms": NOW_MS,
                "ttl_ms": TTL_MS,
                "idempotency_key": None,
            }
        },
        key=f"{method}-{lease['lease_id']}",
    )


def _status(rpc, ok, sock, lease):
    result = _value(
        ok(
            rpc(
                sock,
                "CapacityStatus",
                {
                    "request": {
                        "schema_version": "1",
                        "tenant_ref": TENANT,
                        "cell_id": None,
                        "lease_id": lease["lease_id"],
                        "max_count": 1,
                        "cursor": None,
                    }
                },
            )
        )
    )
    return result["leases"][0], result["transitions"][0]["entries"]


def test_start_and_revoke_are_served_and_refuse_terminal_leases(tmp_path):
    """A served start moves Granted to Running; a served revoke, Running to Revoked."""
    from test_native_no_auth_served import ok, rpc, server

    with server(tmp_path) as sock:
        ok(
            rpc(
                sock,
                "UpdateCapacityCell",
                {
                    "request": {
                        "schema_version": "1",
                        "cell": {
                            "cell_id": CELL,
                            "resource_class": "build_slot",
                            "capacity": 1,
                            "reserved_floor": 0,
                            "epoch": 1,
                            "policy_digest": "sha256:r786",
                            "updated_at_ms": NOW_MS,
                        },
                        "expected_epoch": None,
                        "now_ms": NOW_MS,
                    }
                },
                key="provision-cell",
            )
        )
        granted = _acquire(rpc, ok, sock, "lane-1", "lane")
        assert granted["decision"] == "accepted"
        lease = granted["leases"][0]
        assert lease["state"] == "granted"
        queued = _acquire(rpc, ok, sock, "lane-2", "lane")
        assert queued["leases"][0]["state"] == "queued"
        assert queued["queue_position"] == 1

        started = _value(ok(_mutate(rpc, sock, "StartCapacity", lease)))
        assert started["decision"] == "started"
        assert started["leases"][0]["state"] == "running"
        # A start of a queued lease has no edge and is refused.
        refused = _mutate(rpc, sock, "StartCapacity", queued["leases"][0])
        assert "TRANSITION_REFUSED" in str(refused.get("error"))

        revoked = _value(
            ok(
                rpc(
                    sock,
                    "RevokeCapacity",
                    {
                        "request": {
                            "schema_version": "1",
                            "tenant_ref": TENANT,
                            "lease_id": lease["lease_id"],
                            "reason": "host drained",
                            "now_ms": NOW_MS,
                        }
                    },
                    key="revoke-lane-1",
                )
            )
        )
        assert revoked["decision"] == "revoked"
        assert revoked["leases"][0]["state"] == "revoked"

        # The holder's next renew is the stop signal; the queue head took the slot.
        late = _mutate(rpc, sock, "RenewCapacity", lease)
        text = str(late.get("error"))
        assert "TRANSITION_REFUSED" in text and "terminal_state" in text
        waiting, _ = _status(rpc, ok, sock, queued["leases"][0])
        assert waiting["state"] == "granted"
        final, transitions = _status(rpc, ok, sock, lease)
        assert final["state"] == "revoked"
        assert [entry["event"] for entry in transitions] == [
            "Grant",
            "Start",
            "Revoke",
        ]


def test_the_method_catalog_lists_both_methods_with_their_scopes():
    """Start needs `capacity:lease`; revoke needs `capacity:admin`."""
    engine = pytest.importorskip("medulla.engine")
    catalog = json.loads(engine.method_catalog())
    rows = {row["id"]: row for row in catalog["methods"]}
    assert rows["StartCapacity"]["policy"]["authz_action"] == "capacity:lease"
    assert rows["RevokeCapacity"]["policy"]["authz_action"] == "capacity:admin"
    assert rows["StartCapacity"]["policy"]["mutates"] is True
    assert rows["RevokeCapacity"]["policy"]["mutates"] is True
