"""The Python served-modality record types ``lifecycle`` with the contract enum.

MEDULLA-UNIFIED-DATA-PLANE-R768.3: ``ServedModalityRecord.lifecycle`` is the closed
``LifecycleState`` enum of the engine, not a ``Literal`` of strings, and an illegal
lifecycle change surfaces as the engine's typed ``TransitionRefusedError`` unchanged.
"""

from __future__ import annotations

import enum
import re
import typing
from pathlib import Path

import pytest

from medulla.client import (
    LifecycleState,
    ServedModalityRecord,
    TransitionRefusedError,
    _raise_send_error,
)

ROOT = Path(__file__).resolve().parents[1]
RECORDS = ROOT / "crates/medulla-modality/src/served/records.rs"
pytestmark = [pytest.mark.no_engine]


def _rust_members() -> set[str]:
    source = RECORDS.read_text()
    body = re.search(r"pub enum LifecycleState\s*\{(.*?)\}", source, re.S)
    assert body, "the Rust LifecycleState enum is not where the row names it"
    words = re.findall(r"^\s*([A-Z]\w*)\s*,", body.group(1), re.M)
    return {"_".join(re.findall(r"[A-Z][a-z0-9]*", word)).lower() for word in words}


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R768.3")
def test_lifecycle_is_the_contract_enum_and_refusal_is_typed() -> None:
    assert issubclass(LifecycleState, enum.StrEnum)
    assert {member.value for member in LifecycleState} == _rust_members()
    hints = typing.get_type_hints(ServedModalityRecord)
    assert hints["lifecycle"] is LifecycleState
    assert LifecycleState("tombstoned") is LifecycleState.TOMBSTONED
    with pytest.raises(ValueError):
        LifecycleState("frozen")

    frame = {
        "error": "TRANSITION_REFUSED",
        "error_detail": "LifecycleMachine refused 'TransitionLifecycle'",
        "transition_refused": {
            "machine": "LifecycleMachine",
            "from": LifecycleState.TOMBSTONED.value,
            "event": "TransitionLifecycle",
            "reason": "terminal_state",
        },
    }
    with pytest.raises(TransitionRefusedError) as refused:
        _raise_send_error(frame)
    error = refused.value
    assert (error.machine, error.from_state, error.event, error.reason) == (
        "LifecycleMachine",
        "tombstoned",
        "TransitionLifecycle",
        "terminal_state",
    )
    assert error.from_state == LifecycleState.TOMBSTONED
