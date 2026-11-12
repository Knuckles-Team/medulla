"""Served proof for MEDULLA-ENGINE-PARITY-R009.1: the forms the compatibility
matrix lists as supported are accepted by the live engine."""

from __future__ import annotations

import os

import pytest
from conftest import request_context

from medulla.client import MedullaClient, QueryClient

pytestmark = pytest.mark.spec("MEDULLA-ENGINE-PARITY-R009.1")

# One representative statement per supported read-shaped matrix entry.
SUPPORTED_READS = [
    "SELECT 1 AS one",
    "SELECT md5('a') AS digest",
    "SELECT greatest(1, 2) AS g, least(1, 2) AS l",
]


@pytest.mark.asyncio
async def test_supported_entries_pass() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        query = QueryClient(client)
        for sql in SUPPORTED_READS:
            rows = await query.sql(sql)
            assert len(rows) == 1, sql
    finally:
        await client.close()
