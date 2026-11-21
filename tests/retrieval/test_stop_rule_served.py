"""Served stop rule over successive retrieval rounds (MEDULLA-FEDERATED-QUERY-R082.6).

Each round is a real `RetrievalRound` carrying `sufficiency` and `stop` parts over a
deterministic corpus. The test covers the progressing, saturated and exhausted uncalibrated
outcomes and the calibrated repeat-with-margin stop. No served method reads the in-process
span recorder, so this test asserts the wire form of a terminal verdict: the stop verdict is
terminal, the sufficiency repair is `stop` (no continuation is described), and a further
round sent after it is refused. That no retrieval ran for the refused round is asserted by
the Rust twin `terminal_stop_runs_no_later_retrieval`.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, put, request, run_round

_SCAN = [{"Scan": {"label": "Doc"}}]
_DOCS = ["doc-0", "doc-1"]


def _stop(*, calibrated_rounds: list[int], **overrides: Any) -> dict[str, Any]:
    part: dict[str, Any] = {
        "calibration": {
            "rows": [
                {"round": r, "baseline_confidence": 0.5, "margin_threshold": 0.25}
                for r in calibrated_rounds
            ]
        },
        "history": [],
        "prior_row_ids": [],
        "usage": {"tokens_used": 0, "inference_calls_used": 0, "cost_used": 0},
        "limits": {},
    }
    part.update(overrides)
    return part


def _round(round_number: int, stop: dict[str, Any]) -> dict[str, Any]:
    body = request(_SCAN, stop=stop)
    body["sufficiency"]["round"] = round_number
    return body


def _answer(round_number: int, digest: str, confidence: float) -> dict[str, Any]:
    return {"round": round_number, "answer_digest": digest, "confidence": confidence}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R082.6")
def test_repeat_with_margin_stops(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R082.6."""
    graph = f"stop-{uuid.uuid4().hex[:8]}"
    cl = client()
    try:
        cl.tenants.create(graph)
        for node_id in _DOCS:
            put(cl, graph, node_id, {"type": "Doc", "text": "entry text"})

        # Progressing and uncalibrated: the round continues and names the missing row.
        progressing = run_round(cl, graph, _round(1, _stop(calibrated_rounds=[])))
        verdict = progressing["stop"]
        assert verdict["stop"] is False
        assert verdict["reason"] == "continue_uncalibrated"
        assert verdict["calibration_refusal"] == "ROUND_NOT_CALIBRATED"
        assert progressing["rows"], "a continuing round returns its rows"

        # Saturated and uncalibrated: a measured round that added no evidence is terminal.
        saturated = run_round(
            cl, graph, _round(1, _stop(calibrated_rounds=[], prior_row_ids=_DOCS))
        )
        assert saturated["stop"]["stop"] is True
        assert saturated["stop"]["reason"] == "no_new_evidence"
        assert saturated["stop"]["calibration_refusal"] == "ROUND_NOT_CALIBRATED"

        # Exhausted and uncalibrated: a spent round limit refuses with the dimension.
        with pytest.raises(Exception) as spent:
            run_round(
                cl, graph, _round(4, _stop(calibrated_rounds=[], prior_row_ids=_DOCS))
            )
        assert "RETRIEVAL_BUDGET_EXCEEDED" in str(spent.value)
        assert "round_limit" in str(spent.value)

        # Calibrated: the same answer twice with a margin above the threshold stops.
        repeated = _stop(
            calibrated_rounds=[2],
            history=[_answer(1, "d", 0.6), _answer(2, "d", 0.9)],
        )
        stopped = run_round(cl, graph, _round(2, repeated))
        verdict = stopped["stop"]
        assert verdict["stop"] is True
        assert verdict["reason"] == "answer_repeated_margin_met"
        assert verdict["threshold"] == pytest.approx(0.25)
        assert verdict["margin"] == pytest.approx(0.4)
        # Terminal wire form: no continuation is described.
        assert stopped["sufficiency"]["repair"]["kind"] == "stop"

        # A changed answer continues, and a low margin continues.
        changed = _stop(
            calibrated_rounds=[2],
            history=[_answer(1, "a", 0.6), _answer(2, "b", 0.9)],
        )
        assert (
            run_round(cl, graph, _round(2, changed))["stop"]["reason"]
            == "continue_answer_changed"
        )
        low = _stop(
            calibrated_rounds=[2],
            history=[_answer(1, "d", 0.55), _answer(2, "d", 0.6)],
        )
        assert (
            run_round(cl, graph, _round(2, low))["stop"]["reason"]
            == "continue_margin_low"
        )

        # A further round sent after the terminal verdict is refused: its budget is spent,
        # and the refusal carries no result.
        spent_budget = _stop(
            calibrated_rounds=[2],
            history=[_answer(1, "d", 0.6), _answer(2, "d", 0.9)],
            usage={"tokens_used": 7, "inference_calls_used": 0, "cost_used": 0},
            limits={"token_limit": 7},
        )
        with pytest.raises(Exception) as after:
            run_round(cl, graph, _round(3, spent_budget))
        assert "RETRIEVAL_BUDGET_EXCEEDED" in str(after.value)

        # A forged round past the limit cannot bypass validation.
        with pytest.raises(Exception) as forged:
            run_round(cl, graph, _round(9, _stop(calibrated_rounds=[9])))
        assert "RETRIEVAL_CONTRACT_INVALID" in str(forged.value)
    finally:
        cl.tenants.delete(graph)
