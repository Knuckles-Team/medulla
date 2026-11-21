"""Served terminal and uncalibrated continuations (MEDULLA-FEDERATED-QUERY-R081.8.6).

A continuation carries a ``stop`` part with the caller's calibration table, answer history
and usage. The engine decides the stop in a fixed order: a spent limit first, then a measured
round with no new evidence, and only then ``continue_uncalibrated`` when the round has no
calibration row. A terminal verdict on a spent limit is a refusal that names the exhausted
dimension: the response has no result and returns no continuation. No served method reads
the span recorder, so the proof that nothing was retrieved for the terminal continuation is
the Rust twin ``terminal_feedback_continuation_executes_nothing``.
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


def _seeded(cl: Any, graph: str) -> None:
    cl.tenants.create(graph)
    for node_id in _DOCS:
        put(cl, graph, node_id, {"type": "Doc", "text": "entry text"})


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.8.6")
def test_uncalibrated_continuation_precedence(start_medulla_server) -> None:
    """A round with no calibration row continues only when nothing terminal applies."""
    graph = f"fb-{uuid.uuid4().hex[:8]}"
    cl = client()
    try:
        _seeded(cl, graph)

        # No calibration row and nothing terminal: the round continues, uncalibrated, and
        # the verdict names the missing row.
        progressing = run_round(cl, graph, _round(1, _stop(calibrated_rounds=[])))
        verdict = progressing["stop"]
        assert verdict["stop"] is False
        assert verdict["reason"] == "continue_uncalibrated"
        assert verdict["calibration_refusal"] == "ROUND_NOT_CALIBRATED"
        assert progressing["rows"], "a continuing round returns its rows"

        # A measured round that added no evidence is terminal before calibration is read.
        stale = run_round(
            cl,
            graph,
            _round(1, _stop(calibrated_rounds=[], prior_row_ids=_DOCS)),
        )
        assert stale["stop"]["stop"] is True
        assert stale["stop"]["reason"] == "no_new_evidence"

        # A spent round limit is terminal before either: the response is the refusal that
        # carries the terminal verdict, with no result and no continuation.
        with pytest.raises(Exception) as spent:
            run_round(
                cl,
                graph,
                _round(4, _stop(calibrated_rounds=[], prior_row_ids=_DOCS)),
            )
        message = str(spent.value)
        assert "RETRIEVAL_BUDGET_EXCEEDED" in message
        assert "round_limit" in message

        # Another exhausted budget refuses the same way and names its dimension.
        with pytest.raises(Exception) as tokens:
            run_round(
                cl,
                graph,
                _round(
                    1,
                    _stop(
                        calibrated_rounds=[],
                        usage={
                            "tokens_used": 7,
                            "inference_calls_used": 0,
                            "cost_used": 0,
                        },
                        limits={"token_limit": 7},
                    ),
                ),
            )
        assert "RETRIEVAL_BUDGET_EXCEEDED" in str(tokens.value)
        assert "budget_exhausted" in str(tokens.value)
    finally:
        cl.tenants.delete(graph)
