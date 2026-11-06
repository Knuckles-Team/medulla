"""Live artifacts over the served wire (MEDULLA-FEDERATED-QUERY-R701).

The session fixture in conftest starts and authenticates the native server. The tests
drive the generated `LiveArtifact*` requests and read the refresh records back.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

GOOD_QUERY = "MATCH (:ServedHot)"
BAD_QUERY = "MATCH ("


def _artifact(artifact_id: str, text: str) -> dict:
    return {
        "id": artifact_id,
        "version": 1,
        "template": "Rows: {{0.id}}",
        "source": {"language": "uql", "text": text},
        "data": None,
        "provenance": {},
    }


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _put(client: MedullaClient, artifact: dict) -> dict:
    result = await query.send_live_artifact_put(client, {"artifact": artifact})
    return result.payload


async def _refresh(client: MedullaClient, artifact_id: str, version: int) -> dict:
    result = await query.send_live_artifact_refresh(
        client, {"id": artifact_id, "expected_version": version}
    )
    return result.payload


async def _get(client: MedullaClient, artifact_id: str) -> dict:
    return (await query.send_live_artifact_get(client, {"id": artifact_id})).payload


# spec: MEDULLA-FEDERATED-QUERY-R701.5
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R701.5")
@pytest.mark.asyncio
async def test_refresh_records_success_and_failure() -> None:
    client = await _connect()
    good_id, bad_id = f"good-{uuid.uuid4().hex}", f"bad-{uuid.uuid4().hex}"
    try:
        await _put(client, _artifact(good_id, GOOD_QUERY))
        await _put(client, _artifact(bad_id, BAD_QUERY))

        ok = await _refresh(client, good_id, 1)
        assert ok["new_version"] == 2
        assert ok["record"].get("error") is None

        failed = await _refresh(client, bad_id, 1)
        assert failed["new_version"] == 1
        assert failed["record"]["error"]

        good = await _get(client, good_id)
        assert good["artifact"]["version"] == 2
        assert [r["new_version"] for r in good["refreshes"]] == [2]

        bad = await _get(client, bad_id)
        assert bad["artifact"]["version"] == 1
        assert bad["artifact"]["data"] is None
        assert bad["refreshes"][0]["record"]["error"] == failed["record"]["error"]

        with pytest.raises(RuntimeError, match="LIVE_ARTIFACT_VERSION_CONFLICT"):
            await _refresh(client, good_id, 1)
    finally:
        await client.close()
