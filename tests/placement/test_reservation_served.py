"""Reservation result wire shape: the stored tombstone flag is gone (MEDULLA-PLACEMENT-R781.3).

A terminal reservation is recognised from its state alone. The record and the result carry no
`tombstone` field; a result that still carries one is refused by the client, and the generated
models forbid it. The replay of a terminal row is proved by the store tests.
"""

from __future__ import annotations

import pytest

from medulla.client import _resource_reservation_result
from medulla.generated import models

pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-PLACEMENT-R781.3")]


def _absent_result() -> dict[str, object]:
    return {
        "schema_version": "1",
        "decision": "not_found",
        "reservation_id": "reservation-1",
        "work_item_id": "work-1",
        "attempt": 1,
        "lease_epoch": 1,
        "fencing_token": 1,
        "lifecycle_revision": 0,
        "host_ref": "host-1",
        "host_revision": 1,
        "record": None,
        "state": "absent",
        "held_cpu_weight": 0,
        "held_memory_mib": 0,
        "held_disk_mib": 0,
        "held_process_slots": 0,
        "fairness_debt": 0,
        "changed_work_item_ids": [],
    }


def test_result_has_no_tombstone_field_and_replay_is_idempotent() -> None:
    assert "tombstone" not in models.ResourceReservationResult.model_fields
    assert "tombstone" not in models.ResourceReservationRecord.model_fields
    result = _absent_result()
    assert _resource_reservation_result(result) == result
    # The same result read twice is the same value: nothing in it is a replay marker.
    assert _resource_reservation_result(dict(result)) == result
    with pytest.raises(ValueError):
        _resource_reservation_result({**result, "tombstone": False})
