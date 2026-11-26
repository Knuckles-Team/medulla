"""MEDULLA-MEDIA-VOCAB-R657: the tenant-scoped, content-addressed media store.

A put answers the SHA-256 digest and identical bytes give the same digest. A read by
digest answers the bytes to the holding tenant only: a missing digest, another tenant's
digest, a tombstoned digest and a malformed digest all refuse ``MEDIA_NOT_FOUND`` alike.
A put over the ``media.max_bytes`` knob refuses ``MEDIA_TOO_LARGE`` (proved in the L1
test of ``crates/medulla-server-blob/src/blob/media.rs``) and an empty put or a bad
media type refuses ``MEDIA_INVALID``.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid

import pytest
from conftest import request_context, tenant_request_context

from medulla.client import MedullaClient


async def _connect(context: dict) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        timeout=30,
    )


async def _scenario() -> None:
    owner = await _connect(request_context())
    other = await _connect(
        tenant_request_context(f"media-other-{uuid.uuid4().hex[:8]}")
    )
    try:
        data = f"png-bytes-{uuid.uuid4().hex}".encode()
        digest = await owner.blob.media_put("image/png", data)
        assert digest == hashlib.sha256(data).hexdigest()
        # Idempotent: identical bytes, same digest, still one readable object.
        assert await owner.blob.media_put("image/png", data) == digest
        assert await owner.blob.media_get(digest) == data

        # No existence oracle: foreign, missing and malformed digests refuse alike.
        absent = hashlib.sha256(b"never stored " + data).hexdigest()
        refusals: list[str] = []
        for client, name in ((other, digest), (owner, absent), (owner, "not-a-digest")):
            with pytest.raises(Exception, match="MEDIA_NOT_FOUND") as raised:
                await client.blob.media_get(name)
            refusals.append(str(raised.value))
        assert len(set(refusals)) == 1

        # Another tenant putting the same bytes holds its own copy.
        assert await other.blob.media_put("image/png", data) == digest
        assert await other.blob.media_get(digest) == data

        # A tombstone releases only the caller's hold; repeating it releases nothing.
        assert await owner.blob.media_tombstone(digest) is True
        assert await owner.blob.media_tombstone(digest) is False
        with pytest.raises(Exception, match="MEDIA_NOT_FOUND"):
            await owner.blob.media_get(digest)
        assert await other.blob.media_get(digest) == data

        # Put refusals write nothing.
        with pytest.raises(Exception, match="MEDIA_INVALID"):
            await owner.blob.media_put("image/png", b"")
        with pytest.raises(Exception, match="MEDIA_INVALID"):
            await owner.blob.media_put("png", b"x")
    finally:
        await owner.close()
        await other.close()


# spec: MEDULLA-MEDIA-VOCAB-R657.2
@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R657.1")
@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R657.2")
@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R657.3")
def test_media_put_get_tombstone_are_tenant_scoped_and_have_no_existence_oracle():
    asyncio.run(_scenario())
