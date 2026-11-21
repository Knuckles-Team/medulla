"""Served retrieval arbitration (MEDULLA-FEDERATED-QUERY-R083.6).

A round with an `arbitration` part compares the model prior with the retrieved
answer. Support is classified first: a candidate is supported only when every
claim cites a span the engine verifies on the round's snapshot. A small
confidence gap between two supported candidates abstains with both candidates
in the refusal body; one supported candidate beats an unsupported one whatever
the confidence; a citation of another tenant's document is `unknown`.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from typing import Any

import pytest
from _round_served import client, put, request, run_round

_QUERY_DIGEST = hashlib.sha256(b"what is entry").hexdigest()
_SPAN = b"entry"
_SPAN_DIGEST = hashlib.sha256(_SPAN).hexdigest()
_SCAN = [{"Scan": {"label": "Doc"}}]


def _candidate(grounding: str, confidence: float, cited: str | None) -> dict[str, Any]:
    claims: list[dict[str, Any]] = []
    if cited is not None:
        claims = [
            {
                "text": "claim",
                "citation": {
                    "document_id": cited,
                    "start_byte": 0,
                    "end_byte": len(_SPAN),
                    "expected_digest": _SPAN_DIGEST,
                },
            }
        ]
    return {
        "answer": "entry",
        "confidence": confidence,
        "grounding": grounding,
        "claims": claims,
        "support_state": "unknown",
    }


def _round_with(prior: dict[str, Any], retrieved: dict[str, Any]) -> dict[str, Any]:
    return request(
        _SCAN,
        arbitration={
            "prior": prior,
            "retrieved": retrieved,
            "query_digest": _QUERY_DIGEST,
        },
    )


def _abstention(error: BaseException) -> dict[str, Any]:
    message = str(error)
    assert "ARBITRATION_ABSTAIN" in message
    body, _ = json.JSONDecoder().raw_decode(message[message.index("{") :])
    return body


def _seed(cl: Any, graph: str, ids: list[str]) -> None:
    for node_id in ids:
        put(cl, graph, node_id, {"type": "Doc", "text": "entry text"})


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R083.6")
def test_small_gap_abstains_with_both(start_medulla_server) -> None:
    """Two supported candidates inside the margin abstain and both stay in the body."""
    graph = f"arb-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed(cl, graph, ["doc-1", "doc-2"])
        round_request = _round_with(
            _candidate("parametric", 0.80, "doc-1"),
            _candidate("retrieved", 0.85, "doc-2"),
        )
        with pytest.raises(Exception, match="ARBITRATION_ABSTAIN") as refused:
            run_round(cl, graph, round_request)
        body = _abstention(refused.value)
        verdict = body["verdict"]
        assert verdict["choice"] == "abstain"
        assert verdict["prior"]["support_state"] == "supported"
        assert verdict["retrieved"]["support_state"] == "supported"
        assert verdict["prior"]["answer"] == "entry"
        assert verdict["retrieved"]["answer"] == "entry"
        assert any(p["name"] == "retrieval.arbitration" for p in body["premises"])
    finally:
        cl.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R083.6")
def test_supported_lower_confidence_beats_unsupported_prior(start_medulla_server) -> None:
    """One supported candidate wins even at 0.70 against an unsupported 0.95."""
    graph = f"arb-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed(cl, graph, ["doc-1"])
        result = run_round(
            cl,
            graph,
            _round_with(
                _candidate("parametric", 0.95, None),
                _candidate("retrieved", 0.70, "doc-1"),
            ),
        )
        verdict = result["arbitration"]
        assert verdict["choice"] == "retrieved"
        assert verdict["prior"]["support_state"] == "unsupported"
        assert verdict["retrieved"]["support_state"] == "supported"
    finally:
        cl.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R083.6")
def test_other_tenants_document_is_unknown(start_medulla_server) -> None:
    """A citation of a document the caller's snapshot does not hold is `unknown`."""
    graph = f"arb-{uuid.uuid4().hex[:8]}"
    other = f"arb-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    cl.tenants.create(other)
    try:
        _seed(cl, graph, ["doc-1"])
        _seed(cl, other, ["foreign-doc"])
        with pytest.raises(Exception, match="ARBITRATION_ABSTAIN") as refused:
            run_round(
                cl,
                graph,
                _round_with(
                    _candidate("parametric", 0.90, None),
                    _candidate("retrieved", 0.90, "foreign-doc"),
                ),
            )
        verdict = _abstention(refused.value)["verdict"]
        assert verdict["choice"] == "abstain"
        assert verdict["retrieved"]["support_state"] == "unknown"
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
