"""Served proof for MEDULLA-ENGINE-PARITY-R020.2: the engine reports its capabilities."""

from __future__ import annotations

import os
import re

import pytest
from conftest import request_context

from medulla.client import MedullaClient

pytestmark = pytest.mark.spec("MEDULLA-ENGINE-PARITY-R020.2")


@pytest.mark.asyncio
async def test_report_matches_features() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        report = await client.engine_capabilities()
        again = await client.engine_capabilities()
    finally:
        await client.close()

    assert report["server_version"] == "16.0"
    assert report["index_kinds"] == ["secondary", "partial", "expression", "unique"]
    assert report["constraint_kinds"] == [
        "unique",
        "foreign_key",
        "check",
        "not_null",
        "shacl",
    ]
    assert report["pitr"] is False
    assert report["wire_features"] in (
        [],
        [
            "startup",
            "simple_query",
            "extended_query",
            "prepared_statements",
            "copy_in",
            "copy_out",
            "cancellation",
            "tls",
        ],
    )
    digest = report["compat_matrix_digest"]
    assert re.fullmatch(r"[0-9a-f]{64}", digest)
    assert again["compat_matrix_digest"] == digest
