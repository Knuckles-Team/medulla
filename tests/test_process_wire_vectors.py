"""The process client methods against the Rust wire vectors.

``tests/fixtures/process_wire_vectors.json`` is round-tripped through the Rust ``Method`` and
result types by ``crates/medulla-protocol-types/tests/process_wire_vectors.rs``. Here each
client method must send exactly the vector's params, and the hand-written models must
accept the vector's request.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from pydantic import TypeAdapter

from medulla.client import GraphOperationsClient, MedullaClient
from medulla.generated import models

pytestmark = pytest.mark.no_engine

VECTORS = json.loads(
    (Path(__file__).parent / "fixtures" / "process_wire_vectors.json").read_text()
)
_METHOD: TypeAdapter[Any] = TypeAdapter(models.Method)


class _FakeClient(MedullaClient):
    def __init__(self, answer: dict[str, Any]) -> None:
        self.sent: list[tuple[str, dict[str, Any] | None]] = []
        self._answer = answer

    async def _send(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        graph: str | None = None,
        *,
        idempotency_key: str | None = None,
    ) -> Any:
        self.sent.append((method, params))
        return self._answer


def _expected(key: str) -> tuple[str, dict[str, Any]]:
    request = VECTORS[key]
    return request["method"], request["params"]


@pytest.mark.parametrize(
    "key", [k for k in VECTORS if VECTORS[k].get("method")]
)
def test_models_accept_every_request_vector(key: str) -> None:
    parsed = _METHOD.validate_python(VECTORS[key])
    assert parsed.method == VECTORS[key]["method"]


# spec: MEDULLA-CQ-ORCH-R700.3
@pytest.mark.asyncio
async def test_process_compile_sends_the_vector() -> None:
    transport = _FakeClient(VECTORS["compiled"])
    fake = GraphOperationsClient(transport)
    for key in ("process_compile", "process_compile_first"):
        method, params = _expected(key)
        result = await fake.process_compile(**params)
        assert transport.sent[-1] == (method, params)
        assert result == VECTORS["compiled"]


# spec: MEDULLA-CQ-ORCH-R701.3
@pytest.mark.asyncio
async def test_process_model_import_and_export_send_the_vectors() -> None:
    transport = _FakeClient(VECTORS["imported"])
    fake = GraphOperationsClient(transport)
    method, params = _expected("process_model_import")
    assert await fake.process_model_import(**params) == VECTORS["imported"]
    assert transport.sent[-1] == (method, params)

    transport = _FakeClient(VECTORS["exported"])
    fake = GraphOperationsClient(transport)
    for key in ("process_model_export", "process_model_export_canonical"):
        method, params = _expected(key)
        assert await fake.process_model_export(**params) == VECTORS["exported"]
        assert transport.sent[-1] == (method, params)
