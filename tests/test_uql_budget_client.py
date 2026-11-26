"""Uql result budget client vectors (MEDULLA-DECISION-ENGINE-R144.8).

Static client checks against a fake transport; no engine. The client sends the
caller's budget unchanged, refuses an unknown budget field before sending, and
hands back the engine's cost report on the rows answer without adding one.
"""

from __future__ import annotations

import asyncio

import pytest
from _client_fixtures import PayloadTransport
from pydantic import ValidationError

from medulla.client import QueryClient
from medulla.generated.models import MethodUqlParams

pytestmark = pytest.mark.no_engine

ASKED = {"max_rows": 1_000_000_000, "max_visits": 5, "max_cost_units": 100}
APPLIED = {"max_rows": 10_000, "max_visits": 5, "max_cost_units": 100}
COST = {
    "applied_budget": APPLIED,
    "authorization_revision": 3,
    "cost_used": 2,
    "operation_ids": ["uql:rows"],
    "premises": ["knob:indexless.max_rows=10000"],
    "remaining": {"max_rows": 9_998, "max_visits": 4, "max_cost_units": 98},
    "rows_visited": 2,
    "visits": 1,
}


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R144.8")
def test_uql_sends_the_budget_unchanged_and_returns_the_cost_report() -> None:
    rows = {
        "Rows": {
            "columns": [],
            "rows": [
                {"id": "d1", "score": None, "channels": []},
                {"id": "d2", "score": None, "channels": []},
            ],
            "warnings": [],
            "cost": COST,
        }
    }
    transport = PayloadTransport({"Uql": rows})
    answer = asyncio.run(QueryClient(transport).uql("MATCH (:Doc)", budget=ASKED))
    assert transport.sent[0].method == "Uql"
    assert transport.sent[0].params is not None
    assert transport.sent[0].params["budget"] == ASKED
    assert answer["cost"]["applied_budget"] == APPLIED
    assert answer["cost"]["rows_visited"] == 2


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R144.8")
def test_uql_without_a_budget_sends_none_and_the_request_model_validates() -> None:
    transport = PayloadTransport(
        {"Uql": {"Rows": {"columns": [], "rows": [], "warnings": []}}}
    )
    answer = asyncio.run(QueryClient(transport).uql("MATCH (:Doc)"))
    assert "budget" not in (transport.sent[0].params or {})
    assert "cost" not in answer
    assert MethodUqlParams.model_validate({"text": "x", "budget": ASKED}).budget
    with pytest.raises(ValidationError):
        MethodUqlParams.model_validate(
            {"text": "x", "budget": {**ASKED, "max_seconds": 1}}
        )
