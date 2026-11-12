"""Served release and deployment records (MEDULLA-SDLC-R002.3 to R002.6).

L4: every call goes through the running engine. A release is recorded against
an indexed commit chain; a deployment resolves its release by image digest;
a refusal writes nothing; a recorded release is sealed.
"""

from __future__ import annotations

import os
import uuid

import msgpack
import pytest
from conftest import request_context

GRAPH = "__commons__"


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


async def _connect():
    from medulla.client import MedullaClient

    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _digest(number: int) -> str:
    return f"{number:040x}"


async def _index_chain(client, length: int) -> list[str]:
    """Commit nodes of one linear history, oldest first."""
    digests = [_digest(uuid.uuid4().int % (1 << 150) + n) for n in range(length)]
    for index, digest in enumerate(digests):
        properties = {
            "type": "RepositoryCommit",
            "author": "alice",
            "timestamp_ms": str(index),
            "parents": digests[index - 1] if index else "",
            "changes": "[]",
        }
        await client._send(
            "AddNode",
            {
                "node_id": f"commit:{digest}",
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
            GRAPH,
        )
    return digests


def _release(version: str, head: str, digests: list[str], at: int = 1) -> dict:
    return {
        "repo": f"repo-{version}-{head[:6]}",
        "version": version,
        "git_ref": f"refs/tags/v{version}",
        "changeset_head": head,
        "artifact_digests": digests,
        "published_at": at,
    }


async def _node(client, node_id: str):
    try:
        raw = await client._send("GetNodeProperties", {"node_id": node_id}, GRAPH)
    except Exception:
        return None
    return _body(raw)


async def _record_release(client, release: dict) -> dict:
    return _body(await client._send("RecordRelease", {"release": release}, GRAPH))


async def _record_deployment(client, digest: str, number: int = 0) -> dict:
    deployment = {
        "environment": "prod",
        "workload": f"engine-{number}",
        "image_digest": digest,
        "started_at": number,
        "finished_at": number + 1,
        "outcome": "succeeded",
    }
    return _body(
        await client._send("RecordDeployment", {"deployment": deployment}, GRAPH)
    )


@pytest.mark.spec("MEDULLA-SDLC-R002.3")
@pytest.mark.asyncio
async def test_release_then_deployment_links(start_medulla_server) -> None:
    client = await _connect()
    try:
        chain = await _index_chain(client, 3)
        image = f"sha256:{uuid.uuid4().hex}{uuid.uuid4().hex}"
        release = _release("1.0.0", chain[-1], [image])

        receipt = await _record_release(client, release)
        # spec: MEDULLA-SDLC-R002.3
        assert receipt["release_id"] == f"{release['repo']}@1.0.0"
        assert receipt["contains"] == 3
        assert receipt["replayed"] is False
        stored = await _node(client, receipt["node_id"])
        assert stored["type"] == "Release"
        assert stored["changeset_head"] == chain[-1]

        deployed = await _record_deployment(client, image)
        # spec: MEDULLA-SDLC-R002.3
        assert deployed["release_ref"] == receipt["release_id"]
        assert deployed["outcome"] == "succeeded"
        node = await _node(client, deployed["deployment_id"])
        assert node["release_ref"] == receipt["release_id"]

        unknown = await _record_deployment(client, "sha256:" + "0" * 64, 1)
        # spec: MEDULLA-SDLC-R002.3
        assert unknown["release_ref"] is None
        assert unknown["outcome"] == "unattributed"
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R002.4")
@pytest.mark.asyncio
async def test_release_refusals(start_medulla_server) -> None:
    client = await _connect()
    try:
        chain = await _index_chain(client, 2)
        release = _release("2.0.0", chain[-1], ["sha256:" + "a" * 64])
        first = await _record_release(client, release)

        absent = dict(release, version="2.0.1", changeset_head=_digest(1 << 140))
        with pytest.raises(Exception, match="RELEASE_HEAD_UNKNOWN"):
            await _record_release(client, absent)
        # spec: MEDULLA-SDLC-R002.4
        assert await _node(client, f"release:{release['repo']}:2.0.1") is None

        taken = dict(release, git_ref="refs/tags/other")
        with pytest.raises(Exception, match="RELEASE_VERSION_TAKEN"):
            await _record_release(client, taken)
        # spec: MEDULLA-SDLC-R002.4
        unchanged = await _node(client, first["node_id"])
        assert unchanged["git_ref"] == release["git_ref"]
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R002.5")
@pytest.mark.asyncio
async def test_release_is_sealed(start_medulla_server) -> None:
    client = await _connect()
    try:
        chain = await _index_chain(client, 1)
        release = _release("3.0.0", chain[-1], ["sha256:" + "b" * 64])
        receipt = await _record_release(client, release)
        before = await _node(client, receipt["node_id"])
        # spec: MEDULLA-SDLC-R002.5
        assert before["release_seal"] == receipt["seal"]

        forged = dict(before, git_ref="refs/tags/forged")
        with pytest.raises(Exception, match="RELEASE_SEALED"):
            await client._send(
                "AddNode",
                {
                    "node_id": receipt["node_id"],
                    "properties_msgpack": msgpack.packb(forged, use_bin_type=True),
                },
                GRAPH,
            )
        after = await _node(client, receipt["node_id"])
        # spec: MEDULLA-SDLC-R002.5
        assert after == before

        # An identical replay is idempotent.
        replay = await _record_release(client, release)
        assert replay["replayed"] is True
        assert replay["seal"] == receipt["seal"]
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-SDLC-R002.6")
@pytest.mark.asyncio
async def test_thousand_deployments_link_to_their_releases(
    start_medulla_server,
) -> None:
    client = await _connect()
    try:
        chain = await _index_chain(client, 1)
        suffix = uuid.uuid4().hex
        releases = []
        for number in range(50):
            image = f"sha256:{number:02d}{suffix}"
            release = _release(f"4.0.{number}", chain[-1], [image], at=number)
            release["repo"] = f"fleet-{suffix[:8]}"
            release["changeset_head"] = chain[-1]
            await _record_release(client, release)
            releases.append((image, f"{release['repo']}@4.0.{number}"))
        for number in range(1000):
            image, release_id = releases[number % 50]
            deployed = await _record_deployment(client, image, number)
            # spec: MEDULLA-SDLC-R002.6
            assert deployed["release_ref"] == release_id
    finally:
        await client.close()
