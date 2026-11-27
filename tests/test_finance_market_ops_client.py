"""MEDULLA-FINANCE-PRIMITIVES-R006 and R007.1 wire mirrors of the ``FinanceMarket`` ops.

L1: the generated models accept the ``corporate_actions`` and ``strategy_evaluate`` ops
and their results and refuse anything else, with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla.generated import models

DAY = 86_400_000_000_000


def _action(kind: dict, revision: int = 0) -> dict:
    return {
        "listing_id": "L1",
        "action": kind,
        "effective_time": DAY,
        "known_at": DAY,
        "source": "fixture",
        "revision": revision,
    }


# spec: MEDULLA-FINANCE-PRIMITIVES-R006, MEDULLA-FINANCE-PRIMITIVES-R006.1
@pytest.mark.spec("MEDULLA-FINANCE-PRIMITIVES-R006")
@pytest.mark.spec("MEDULLA-FINANCE-PRIMITIVES-R006.1")
def test_the_corporate_actions_op_mirrors_the_wire_shape() -> None:
    adapter = TypeAdapter(models.FinanceMarketOp)
    op = {
        "op": "corporate_actions",
        "as_of": 3 * DAY,
        "records": [
            _action({"kind": "dividend", "amount_ticks": 50}),
            _action({"kind": "split", "from": 1, "to": 2}, 1),
            _action({"kind": "symbol_change", "new_ticker": "NEWX"}),
            _action({"kind": "delisting"}),
        ],
    }
    parsed = adapter.validate_python(op)
    assert isinstance(parsed, models.FinanceMarketOpCorporateActions)
    assert len(parsed.records) == 4
    bare = adapter.validate_python({"op": "corporate_actions", "records": []})
    assert isinstance(bare, models.FinanceMarketOpCorporateActions)
    assert bare.as_of is None
    with pytest.raises(ValidationError):
        adapter.validate_python(
            {"op": "corporate_actions", "records": [_action({"kind": "merger"})]}
        )


# spec: MEDULLA-FINANCE-PRIMITIVES-R007.1, MEDULLA-FINANCE-PRIMITIVES-R006.1
@pytest.mark.spec("MEDULLA-FINANCE-PRIMITIVES-R007.1")
@pytest.mark.spec("MEDULLA-FINANCE-PRIMITIVES-R006.1")
def test_the_strategy_evaluate_op_and_its_result_mirror_the_wire_shape() -> None:
    adapter = TypeAdapter(models.FinanceMarketOp)
    op = {
        "op": "strategy_evaluate",
        "spec": {
            "version": 1,
            "kind": {"DollarCostAverageFixedAmount": {"amount_minor_units": 10_000}},
        },
    }
    parsed = adapter.validate_python(op)
    assert isinstance(parsed, models.FinanceMarketOpStrategyEvaluate)
    action = models.ProposedAction.model_validate(
        {"strategy_version": 1, "side": "Buy", "amount_minor_units": 10_000}
    )
    assert action.side is models.OrderSide.BUY
    with pytest.raises(ValidationError):
        adapter.validate_python({**op, "spec": {"version": 1, "kind": {}}})
