"""MEDULLA-OBSERVE-R006.6: the Python client reads every introspection section."""

from __future__ import annotations

import asyncio
import os

import pytest
from conftest import request_context

from medulla.client import MedullaClient

SECTIONS = (
    "requests",
    "tasks",
    "runtime",
    "locks",
    "readiness",
    "startup",
    "build",
    "events",
)


async def _scenario() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        for section in SECTIONS:
            page = await client.introspect(section)
            assert page["section"] == section
            assert isinstance(page["rows"], list)
            assert "seq" in page
        # The build section is one row naming the running binary.
        build = (await client.introspect("build"))["rows"]
        assert len(build) == 1
        assert build[0]["version"]
        assert build[0]["git_sha"]
        # The events section holds the transitions of this very session's requests.
        events = (await client.introspect("events", limit=5))["rows"]
        assert len(events) <= 5
        assert all(row["machine"] and row["to"] for row in events)
        # A limit outside 1..=500 and a malformed cursor are refused with their codes.
        with pytest.raises(Exception, match="INTROSPECT_LIMIT_INVALID"):
            await client.introspect("tasks", limit=0)
        with pytest.raises(Exception, match="INTROSPECT_LIMIT_INVALID"):
            await client.introspect("tasks", limit=501)
        with pytest.raises(Exception, match="INTROSPECT_CURSOR_INVALID"):
            await client.introspect("tasks", cursor="not-a-task-id")
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-OBSERVE-R006.6")
def test_client_reads_each_section():
    asyncio.run(_scenario())
