"""MEDULLA-OBSERVE-R002.5: the trace id a client sends is the one the engine shows."""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient


async def _events(client: MedullaClient) -> list[dict[str, object]]:
    """Every transition event the engine still holds, oldest first."""
    rows: list[dict[str, object]] = []
    cursor: str | None = None
    while True:
        page = await client.introspect("events", limit=500, cursor=cursor)
        rows.extend(page["rows"])
        cursor = page.get("next_cursor")
        if cursor is None:
            return rows


async def _scenario() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        trace = f"00-{uuid.uuid4().hex}-{uuid.uuid4().hex[:16]}-01"
        await client.call("Ping", None, trace_id=trace)
        mine = [
            row
            for row in await _events(client)
            if row["machine"] == "RequestMachine" and row["trace_id"] == trace
        ]
        # The request walked its machine under the trace the caller sent.
        assert {row["to"] for row in mine} >= {"admitted", "done"}
        # A call without a trace id leaves no row under this one.
        await client.call("Ping", None)
        again = [
            row
            for row in await _events(client)
            if row["machine"] == "RequestMachine" and row["trace_id"] == trace
        ]
        assert len(again) == len(mine)
        # An over-long trace id is refused before any request is sent.
        with pytest.raises(ValueError):
            await client.call("Ping", None, trace_id="0" * 56)
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-OBSERVE-R002.5")
def test_client_trace_id_appears_in_server_log():
    asyncio.run(_scenario())
