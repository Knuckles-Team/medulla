"""Served `SourceIngest` tag features: MEDULLA-MEDIA-VOCAB-R001.100.2."""

from __future__ import annotations

import copy
from typing import Any

import msgpack
import pytest

from medulla.client import EngineResponseError
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

ITEM_REVISION = 3


def _tag_value(media_revision: int, producer_id: str = "producer-1") -> bytes:
    return msgpack.packb(
        {
            "label": "harbor",
            "origin": "model",
            "taxonomy_id": "scene-taxonomy",
            "taxonomy_version": "1",
            "tag_kind": "object",
            "confidence": 0.9,
            "producer_id": producer_id,
            "evidence_ref": "evidence://frame-1",
            "media_revision": media_revision,
        }
    )


def _feature(feature_id: str, object_id: str, media_revision: int, **kwargs: Any) -> dict:
    return {
        "feature_id": feature_id,
        "object_id": object_id,
        "kind": "media_tag",
        "value_msgpack": list(_tag_value(media_revision, **kwargs)),
        "model_version": "model-7",
    }


def _tagged(
    connector: str, page: int, previous: int | None, features: list[dict]
) -> dict[str, Any]:
    """The item record at `ITEM_REVISION` plus `features` in one batch."""
    batch = copy.deepcopy(_batch(connector, page, previous, "item"))
    batch["strict_schema"] = False
    batch["records"][0]["payload"]["revision"] = ITEM_REVISION
    batch["features"] = features
    return batch


# spec: MEDULLA-MEDIA-VOCAB-R001.100.2
@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R001.100.2")
@pytest.mark.asyncio
async def test_tag_ingest_refusals_and_replay() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        seeded = await _submit(client, _tagged(connector, 1, None, []), "seed")
        assert seeded.disposition is SourceIngestionDisposition.COMMITTED
        items = await client.nodes.list_by_label("Document", 0)
        assert len(items) == 1
        item_id = items[0][0]

        async def refused(features: list[dict], code: str) -> None:
            with pytest.raises(EngineResponseError) as error:
                await _submit(client, _tagged(connector, 2, 1, features), code)
            assert code in str(error.value)
            assert await client.nodes.list_by_label("MediaTag", 0) == []

        # An empty producer, a stale revision and an unknown object each refuse and
        # leave the store unchanged.
        await refused(
            [
                {
                    **_feature("f-empty", item_id, ITEM_REVISION, producer_id=""),
                }
            ],
            "TAG_PROVENANCE_MISSING",
        )
        await refused(
            [_feature("f-stale", item_id, ITEM_REVISION - 1)],
            "TAG_MEDIA_REVISION_STALE",
        )
        await refused(
            [_feature("f-unknown", "no-such-media-item", ITEM_REVISION)],
            "TAG_MEDIA_REVISION_STALE",
        )

        accepted = _tagged(
            connector, 2, 1, [_feature("f-ok", item_id, ITEM_REVISION)]
        )
        first = await _submit(client, accepted, "tag")
        assert first.disposition is SourceIngestionDisposition.COMMITTED
        tags = await client.nodes.list_by_label("MediaTag", 0)
        assert len(tags) == 1
        tag_id = tags[0][0]
        assert await client.edges.has(tag_id, item_id)

        again = await _submit(client, accepted, "tag-again")
        assert again.disposition is SourceIngestionDisposition.REPLAYED
        assert again.batch_digest == first.batch_digest
        assert len(await client.nodes.list_by_label("MediaTag", 0)) == 1

        # A batch with 1025 features is refused at decode.
        oversized = _tagged(
            connector,
            3,
            2,
            [_feature(f"f-{index}", item_id, ITEM_REVISION) for index in range(1025)],
        )
        with pytest.raises((ValueError, EngineResponseError)):
            await _submit(client, oversized, "oversized")
    finally:
        await _close(client, graph)
