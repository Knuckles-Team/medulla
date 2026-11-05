"""The generated client has exactly one sender for every served method."""

from __future__ import annotations

import ast
import asyncio
import re
import uuid
from collections.abc import Callable, Coroutine
from pathlib import Path
from typing import Any, cast

import pytest
from _decision_resident_fixture import publish, request, schema_body

import medulla.generated as generated
from medulla.generated import METHOD_IDS, SEND_BY_METHOD

ROOT = Path(__file__).resolve().parents[1]
METHOD_DIR = ROOT / "crates/medulla-protocol-types/src/protocol/method"
GENERATED_DIR = ROOT / "medulla/generated"
# A variant opens at the enum's four-space indent inside a `__eg_method_chunk_*` macro.
_VARIANT = re.compile(r"^    ([A-Z][A-Za-z0-9]*)\s*(?:\{|\(|,|$)", re.M)


def method_variants() -> set[str]:
    variants: set[str] = set()
    for path in sorted(METHOD_DIR.glob("method_*.rs")):
        variants.update(_VARIANT.findall(path.read_text()))
    return variants


def _imports(path: Path) -> set[str]:
    names: set[str] = set()
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.Import):
            names.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            names.add(node.module)
    return names


def _sender(method: str) -> Callable[..., Coroutine[Any, Any, Any]]:
    return cast(Callable[..., Coroutine[Any, Any, Any]], SEND_BY_METHOD[method])


class _AsyncWire:
    """The generated senders await `_send`; the served fixture client is sync."""

    def __init__(self, client: Any) -> None:
        self._client = client

    async def _send(self, method, params, graph, *, idempotency_key=None):
        return self._client._send(method, params, graph)


# spec: MEDULLA-DECISION-ENGINE-R116.2
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R116.2")
def test_every_method_has_one_generated_sender(clean_graph) -> None:
    variants = method_variants()
    assert len(variants) > 400, "the variant scan found the Method enum"
    assert sorted(variants - set(SEND_BY_METHOD)) == [], "variants without a sender"
    assert sorted(set(SEND_BY_METHOD) - variants) == [], "senders without a variant"
    assert set(METHOD_IDS) == set(SEND_BY_METHOD)
    senders = [getattr(fn, "__qualname__", "") for fn in SEND_BY_METHOD.values()]
    assert len(set(senders)) == len(senders), "one sender serves one method only"
    for path in sorted(GENERATED_DIR.glob("*.py")):
        imported = _imports(path)
        assert not any(
            name == "medulla.engine" or name.startswith("medulla.engine.")
            for name in imported
        ), f"{path.name} imports medulla.engine"
    assert generated.send_by_id is not None

    wire = _AsyncWire(clean_graph)
    count = asyncio.run(_sender("NodeCount")(wire))
    assert count == 0
    assert asyncio.run(generated.send_by_id(wire, "NodeCount")) == 0
    schema = publish(
        clean_graph,
        "inventory-schema-" + uuid.uuid4().hex,
        "feature_schema",
        schema_body(),
    )
    decided = asyncio.run(_sender("Decide")(wire, {"request": request(schema)}))
    assert decided.method == "Decide"
    assert decided.payload["records"], (
        "the served decision call returned a sealed record"
    )
    with pytest.raises(ValueError):
        asyncio.run(generated.send_by_id(wire, "NotAMethod"))
