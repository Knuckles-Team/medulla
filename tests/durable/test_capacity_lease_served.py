"""Served proof for MEDULLA-DURABLE-KERNEL-R867: capacity cells and leases.

One real native engine owns a capacity cell with a reserved floor. Leases of
each priority are acquired through the served methods: only
`background_ingestion` is denied the floor, a replay under the same
idempotency key returns the stored lease, a stale fence and an expired lease
refuse, and another owner or another tenant can neither take nor change a
lease it does not hold.
"""

import hashlib

import msgpack
import pytest

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
    pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R867"),
]

CELL = "cell-r867"
TENANT = "tenant-a"
OTHER_TENANT = "tenant-b"
NOW_MS = 1_000_000
TTL_MS = 60_000
CAPACITY = 10
FLOOR = 4
# The anonymous local principal's persistence id, as the engine derives it.
OWNER = "principal:sha256:" + hashlib.sha256(b"anonymous").hexdigest()
STRANGER = "principal:sha256:" + hashlib.sha256(b"stranger").hexdigest()


def _value(result):
    """A served result is a value, or the MessagePack of one."""
    if isinstance(result, (bytes, bytearray)):
        return msgpack.unpackb(result, raw=False)
    return result


def _acquire(
    rpc, ok, sock, key, priority, amount, *, tenant=TENANT, owner=OWNER, now_ms=NOW_MS
):
    return _value(
        ok(
            rpc(
                sock,
                "AcquireCapacity",
                {
                    "request": {
                        "schema_version": "1",
                        "tenant_ref": tenant,
                        "work_item_id": f"wi-{key}",
                        "owner_digest": owner,
                        "idempotency_key": key,
                        "priority": priority,
                        "demands": [
                            {
                                "cell_id": CELL,
                                "resource_class": "worker",
                                "amount": amount,
                            }
                        ],
                        "lease_id": None,
                        "ttl_ms": TTL_MS,
                        "now_ms": now_ms,
                        "cost_budget_micros": None,
                        "token_budget": None,
                    }
                },
                key=key,
            )
        )
    )


def _mutate(
    rpc, sock, method, lease, *, tenant=TENANT, owner=OWNER, now_ms=NOW_MS, fence=None
):
    return rpc(
        sock,
        method,
        {
            "request": {
                "schema_version": "1",
                "tenant_ref": tenant,
                "owner_digest": owner,
                "leases": [
                    {
                        "lease_id": lease["lease_id"],
                        "lease_epoch": lease["lease_epoch"],
                        "fence_token": lease["fence_token"] if fence is None else fence,
                    }
                ],
                "now_ms": now_ms,
                "ttl_ms": TTL_MS,
                "idempotency_key": None,
            }
        },
        key=f"{method}-{lease['lease_id']}-{now_ms}-{tenant}-{fence}",
    )


def test_background_lease_cannot_spend_reserved_floor(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R867.

    Priorities, floor, replay, fences, expiry and ownership over the wire.
    """
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
                            "resource_class": "worker",
                            "capacity": CAPACITY,
                            "reserved_floor": FLOOR,
                            "epoch": 1,
                            "policy_digest": "sha256:r867",
                            "updated_at_ms": NOW_MS,
                        },
                        "expected_epoch": None,
                        "now_ms": NOW_MS,
                    }
                },
                key="provision-cell",
            )
        )

        # Background work spends only the spare capacity (10 - floor 4 = 6).
        spare = _acquire(rpc, ok, sock, "bg-1", "background_ingestion", 6)
        assert spare["decision"] == "accepted"
        lease = spare["leases"][0]
        denied = _acquire(rpc, ok, sock, "bg-2", "background_ingestion", 1)
        assert denied["decision"] == "exhausted"
        assert denied["leases"] == []
        assert denied["available"][0]["available"] == 0

        # A replay under the same key returns the stored lease.
        replay = _acquire(rpc, ok, sock, "bg-1", "background_ingestion", 6)
        assert replay["decision"] == "replayed"
        assert replay["leases"][0]["lease_id"] == lease["lease_id"]

        # Every other priority may draw on the floor.
        floor = _acquire(rpc, ok, sock, "ia-1", "interactive", FLOOR)
        assert floor["decision"] == "accepted"
        assert (
            _acquire(rpc, ok, sock, "ia-2", "interactive", 1)["decision"] == "exhausted"
        )

        # A stale fence refuses, and nothing changes.
        stale = _value(
            ok(
                _mutate(
                    rpc, sock, "RenewCapacity", lease, fence=lease["fence_token"] + 1
                )
            )
        )
        assert stale["decision"] == "stale_fence"

        # Another owner is refused before the ledger; another tenant cannot
        # renew or release a lease it does not hold.
        other_owner = _mutate(rpc, sock, "ReleaseCapacity", lease, owner=STRANGER)
        assert "ACCESS_DENIED" in str(other_owner.get("error"))
        cross = _value(
            ok(_mutate(rpc, sock, "RenewCapacity", lease, tenant=OTHER_TENANT))
        )
        assert cross["decision"] == "stale_fence"
        assert cross["leases"] == []

        # An expired lease refuses to renew.
        expired = _value(
            ok(_mutate(rpc, sock, "RenewCapacity", lease, now_ms=NOW_MS + TTL_MS + 1))
        )
        assert expired["decision"] == "expired"

        # A live lease renews and releases; the released spare is spendable again.
        renewed = _value(
            ok(_mutate(rpc, sock, "RenewCapacity", lease, now_ms=NOW_MS + 10))
        )
        assert renewed["decision"] == "renewed"
        released = _value(
            ok(
                _mutate(
                    rpc,
                    sock,
                    "ReleaseCapacity",
                    renewed["leases"][0],
                    now_ms=NOW_MS + 20,
                )
            )
        )
        assert released["decision"] == "released"
        again = _acquire(
            rpc, ok, sock, "bg-3", "background_ingestion", 1, now_ms=NOW_MS + 30
        )
        assert again["decision"] == "accepted"
