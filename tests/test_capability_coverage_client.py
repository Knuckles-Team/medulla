"""MEDULLA-DECISION-ENGINE-R126.2.2: the generated client exposes CapabilityCoverage."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import pytest

from medulla.generated import storage

pytestmark = pytest.mark.no_engine

_METHOD_ENUM = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "medulla-protocol-types"
    / "src"
    / "protocol"
    / "method"
)


class _FakeClient:
    def __init__(self) -> None:
        self.sent: list[tuple[str, Any]] = []

    async def _send(self, method: str, params: Any, graph: Any, **_: Any) -> Any:
        self.sent.append((method, params))
        return {}


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R126.2.2")
def test_generated_client_exposes_capability_coverage() -> None:
    assert callable(storage.send_capability_coverage)
    assert callable(storage.decode_capability_coverage)
    # The Rust `Method` enum is the source of truth (the committed contract
    # file was retired), so the variant is looked up in its source.
    assert any(
        "    CapabilityCoverage {" in path.read_text()
        for path in sorted(_METHOD_ENUM.glob("method_*.rs"))
    )


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R126.2.2")
def test_capability_coverage_send_validates_and_dispatches() -> None:
    client = _FakeClient()
    params = {
        "request": {
            "candidates": {"classification_under": None, "kinds": []},
            "requirements": {"topology": None},
            "tenant_id": "0" * 64,
        }
    }
    asyncio.run(storage.send_capability_coverage(client, params))
    assert client.sent[0][0] == "CapabilityCoverage"
