"""Pairwise candidate ranking of a task plan over a real authenticated loopback engine.

MEDULLA-DECISION-ENGINE-R140.103: the ranking runs only under the one pinned
aggregation policy (`pairwise_copeland`, revision 1). A request that selects no
policy, another policy or another revision of it is refused with
``PAIRWISE_POLICY_UNPINNED`` and ranks nothing.
"""

import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body

from decide.test_task_plan_served import (  # noqa: F401  (fixture re-export)
    authenticated_plan_client,
)
from decide.test_trajectory_refinement_served import _ask, _judged, _pairwise

pytestmark = pytest.mark.no_engine  # Each test owns its actual loopback-TCP engine.

_RECORDS = [
    _judged("a", "b", "win_a"),
    _judged("a", "c", "win_a"),
    _judged("b", "c", "win_b"),
]


def _with_policy(policy: dict[str, Any] | None) -> dict[str, Any]:
    """A complete pairwise query whose aggregation policy is `policy` (none: absent)."""
    query = _pairwise(_RECORDS, pinned=False)
    if policy is not None:
        query["policy"] = policy
    return query


# spec: MEDULLA-DECISION-ENGINE-R140.103
def test_refuses_unselected_aggregation_policy(
    authenticated_plan_client,  # noqa: F811  (the re-exported fixture)
):
    client = authenticated_plan_client
    try:
        pin = publish(
            client,
            "pairwise-schema-" + uuid.uuid4().hex,
            "feature_schema",
            schema_body(),
        )
        for policy in (
            None,
            {"id": "borda_count", "revision": 1},
            {"id": "pairwise_copeland", "revision": 2},
        ):
            with pytest.raises(Exception, match="PAIRWISE_POLICY_UNPINNED"):
                client._send("Decide", {"request": _ask(pin, _with_policy(policy))})
    finally:
        client.close()
