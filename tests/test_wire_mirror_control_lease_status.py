"""Wire mirrors carry a machine state as the closed enum of its Rust owner.

MEDULLA-PLACEMENT-R785.2: the control lease views type `status` as the
`ControlLeaseStatus` enum, mirrored from the Rust `ControlLeaseStatus`
(`crates/medulla-work-types/src/control_lease.rs`); a word outside the enum is
refused when decoded, and the mirror holds no transition function.
"""

from __future__ import annotations

import re
from enum import Enum
from pathlib import Path

import pytest
from pydantic import ValidationError

from medulla.generated import models

RUST_SOURCE = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "medulla-work-types"
    / "src"
    / "control_lease.rs"
)


def _rust_variants(enum_name: str) -> list[str]:
    text = RUST_SOURCE.read_text(encoding="utf-8")
    body = text.split(f"pub enum {enum_name} {{", 1)[1].split("}", 1)[0]
    names = [
        line.strip().rstrip(",")
        for line in body.splitlines()
        if line.strip() and not line.strip().startswith(("//", "#"))
    ]
    return [re.sub(r"(?<!^)([A-Z])", r"_\1", name).lower() for name in names]


def _view(status: str) -> dict[str, object]:
    return {
        "expires_at_ms": 2,
        "grant": {},
        "hard_expires_at_ms": 3,
        "issued_at_ms": 1,
        "kind": "k",
        "lease_id": "l",
        "revision": 0,
        "status": status,
    }


# spec: MEDULLA-PLACEMENT-R785.2
def test_control_lease_views_type_status_as_the_rust_enum() -> None:
    status = models.ControlLeaseStatus
    assert issubclass(status, Enum)
    assert [member.value for member in status] == _rust_variants("ControlLeaseStatus")

    for word in _rust_variants("ControlLeaseStatus"):
        view = models.ControlLeaseView.model_validate(_view(word))
        assert view.status is status(word)
        request = models.ListControlLeasesRequest.model_validate(
            {"limit": 1, "tenant": "t", "status": word}
        )
        assert request.status is status(word)

    with pytest.raises(ValidationError):
        models.ControlLeaseView.model_validate(_view("paused"))
    with pytest.raises(ValidationError):
        models.ListControlLeasesRequest.model_validate(
            {"limit": 1, "tenant": "t", "status": "paused"}
        )

    # The mirror writes no state: no transition function on the views.
    for cls in (models.ControlLeaseView, models.ListControlLeasesRequest):
        assert not any(name in vars(cls) for name in ("apply", "transition"))
