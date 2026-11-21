"""`MedullaClient.call` sends any served method by id (MEDULLA-CONTRACT-R059.1)."""

from __future__ import annotations

from typing import Any

import pytest

from medulla import generated
from medulla.client import MedullaClient

pytestmark = pytest.mark.no_engine


def _client_without_transport() -> MedullaClient:
    client = object.__new__(MedullaClient)

    async def _never(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("the transport must not be used")

    client._send = _never  # type: ignore[method-assign]
    return client


@pytest.mark.spec("MEDULLA-CONTRACT-R059.1")
@pytest.mark.asyncio
async def test_call_routes_to_the_sender_and_refuses_unknown_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # spec: MEDULLA-CONTRACT-R059.1
    client = _client_without_transport()
    seen: list[tuple[Any, ...]] = []

    async def sender(
        target: Any,
        params: dict[str, Any] | None,
        graph: str | None,
        *,
        idempotency_key: str | None = None,
    ) -> str:
        seen.append((target, params, graph, idempotency_key))
        return "sent"

    monkeypatch.setitem(generated.SEND_BY_METHOD, "CreateGraph", sender)
    result = await client.call(
        "CreateGraph", {"name": "g"}, "g", idempotency_key="k"
    )
    assert result == "sent"
    assert seen == [(client, {"name": "g"}, "g", "k")]

    with pytest.raises(ValueError, match="NoSuchMethod"):
        await client.call("NoSuchMethod")
