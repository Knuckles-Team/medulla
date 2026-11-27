"""MEDULLA-OBSERVE-R001.4: a refusal reaches the exception with its cause reference."""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError, _raise_send_error


@pytest.mark.spec("MEDULLA-OBSERVE-R001.4")
def test_partial_materialization_and_cause_ref_reach_the_exception():
    # A typed refusal keeps its code and its JSON detail; it has no cause reference.
    detail = '{"phase":"partial","retryable":true}'
    with pytest.raises(EngineResponseError) as typed:
        _raise_send_error({"error": "PARTIAL_MATERIALIZATION", "error_detail": detail})
    assert typed.value.code == "PARTIAL_MATERIALIZATION"
    assert typed.value.detail == detail
    assert typed.value.cause_ref is None

    # An uncoded refusal arrives as INTERNAL with the cause of the one engine WARN line.
    with pytest.raises(EngineResponseError) as uncoded:
        _raise_send_error(
            {
                "error": "INTERNAL",
                "error_detail": "unclassified engine refusal (cause-0badc0de)",
            }
        )
    assert uncoded.value.code == "INTERNAL"
    assert uncoded.value.cause_ref == "cause-0badc0de"
    assert "cause-0badc0de" in str(uncoded.value)

    # Only an INTERNAL refusal carries a cause reference, in the engine's shape.
    assert EngineResponseError("INTERNAL", "boom").cause_ref is None
    assert EngineResponseError("INTERNAL", None).cause_ref is None
    assert (
        EngineResponseError("GRAPH_NOT_FOUND", "kg (cause-0badc0de)").cause_ref is None
    )
    assert EngineResponseError("INTERNAL", "x (cause-ZZZZZZZZ)").cause_ref is None
