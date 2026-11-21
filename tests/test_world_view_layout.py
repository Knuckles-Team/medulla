"""Derived layout, stored overrides (MEDULLA-WORLD-VIEW-R002)."""

from __future__ import annotations

import asyncio
from typing import Any

import msgpack
import pytest
from _client_fixtures import RecordingTransport, SentCall

from medulla.world_view import (
    DISTRICT_PITCH,
    PLOT_PITCH,
    CatalogEntry,
    Override,
    OverrideStore,
    WorldBuilding,
    WorldDistrict,
    apply_overrides,
    derive_placements,
    read_world_envelope,
)

pytestmark = pytest.mark.no_engine


class _Graph(RecordingTransport):
    def reply(self, call: SentCall) -> Any:
        assert call.params is not None
        if call.method == "AddNode":
            return call.params["node_id"]
        return []


CATALOG = [
    CatalogEntry("zeta", "media"),
    CatalogEntry("alpha", "media"),
    CatalogEntry("mid", ""),
    CatalogEntry("beta", "finance"),
]


# spec: MEDULLA-WORLD-VIEW-R002
@pytest.mark.spec("MEDULLA-WORLD-VIEW-R002")
def test_derived_placement_is_never_stored() -> None:
    transport = _Graph()

    derived = derive_placements(CATALOG)
    assert derive_placements(list(reversed(CATALOG))) == derived
    assert [(p.server_id, p.district, p.plot) for p in derived] == [
        ("beta", "finance", 0),
        ("alpha", "media", 0),
        ("zeta", "media", 1),
        ("mid", "ungrouped", 0),
    ]
    assert (derived[2].x, derived[2].z) == (DISTRICT_PITCH + PLOT_PITCH, 0)

    grown = derive_placements([*CATALOG, CatalogEntry("aardvark", "finance")])
    assert grown != derived
    assert transport.sent == [], "derived placement writes nothing"

    with pytest.raises(ValueError, match="WORLD_VIEW_DUPLICATE_SERVER: beta"):
        derive_placements([*CATALOG, CatalogEntry("beta", "x")])
    with pytest.raises(ValueError, match="WORLD_VIEW_REASON_REQUIRED"):
        Override("beta", 1, 2, "  ")

    store = OverrideStore(transport)  # type: ignore[arg-type]
    override = Override("alpha", 5, 7, "moved next to the harbor")
    asyncio.run(store.put(override))
    assert [call.method for call in transport.sent] == ["AddNode"]
    params = transport.sent[0].params
    assert params is not None
    assert params["node_id"] == "world:override:alpha"
    assert msgpack.unpackb(params["properties_msgpack"], raw=False) == {
        "type": "Building",
        "server_id": "alpha",
        "position_x": 5,
        "position_z": 7,
        "override_reason": "moved next to the harbor",
    }

    layout = apply_overrides(derived, [override])
    moved = next(p for p in layout.placements if p.server_id == "alpha")
    assert (moved.x, moved.z) == (5, 7)
    assert (moved.district, moved.plot) == ("media", 0)
    assert layout.stale_overrides == ()

    remaining = [entry for entry in CATALOG if entry.server_id != "alpha"]
    stale = apply_overrides(derive_placements(remaining), [override])
    assert stale.stale_overrides == ("alpha",)
    assert all(p.server_id != "alpha" for p in stale.placements)


class _Stored(RecordingTransport):
    """A fake graph that keeps what `AddNode` wrote and serves it back by label."""

    def __init__(self) -> None:
        super().__init__()
        self.nodes: dict[str, bytes] = {}

    def reply(self, call: SentCall) -> Any:
        assert call.params is not None
        if call.method == "AddNode":
            self.nodes[call.params["node_id"]] = call.params["properties_msgpack"]
            return call.params["node_id"]
        if call.method == "GetNodesByLabel":
            return [[node_id, raw] for node_id, raw in self.nodes.items()]
        return []


# spec: MEDULLA-WORLD-VIEW-R003
@pytest.mark.spec("MEDULLA-WORLD-VIEW-R003")
def test_world_envelope_is_derived_overridden_and_read_only() -> None:
    transport = _Stored()
    store = OverrideStore(transport)  # type: ignore[arg-type]
    catalog = [
        CatalogEntry("zeta", "media"),
        CatalogEntry("alpha", "media"),
        CatalogEntry("beta", "finance"),
    ]

    envelope = asyncio.run(read_world_envelope(catalog, store))
    assert envelope.version == 1
    assert envelope.districts == (WorldDistrict("finance", 0), WorldDistrict("media", 1))
    assert envelope.buildings == (
        WorldBuilding("beta", "finance", 0, 0, 0, False, None),
        WorldBuilding("alpha", "media", 0, DISTRICT_PITCH, 0, False, None),
        WorldBuilding("zeta", "media", 1, DISTRICT_PITCH + PLOT_PITCH, 0, False, None),
    )
    assert envelope.stale_overrides == ()
    assert [call.method for call in transport.sent] == ["GetNodesByLabel"]
    assert transport.nodes == {}, "reading the envelope writes nothing"

    asyncio.run(store.put(Override("alpha", 5, 7, "moved next to the harbor")))
    asyncio.run(store.put(Override("ghost", 1, 1, "its connector is gone")))
    moved = asyncio.run(read_world_envelope(catalog, store))
    alpha = next(b for b in moved.buildings if b.server_id == "alpha")
    assert (alpha.x, alpha.z) == (5, 7)
    assert alpha.overridden and alpha.reason == "moved next to the harbor"
    assert all(
        not b.overridden and b.reason is None for b in moved.buildings if b.server_id != "alpha"
    )
    assert moved.stale_overrides == ("ghost",)
    assert all(b.server_id != "ghost" for b in moved.buildings)

    writes_before = [call.method for call in transport.sent].count("AddNode")
    asyncio.run(read_world_envelope(catalog, store))
    assert [call.method for call in transport.sent].count("AddNode") == writes_before

    empty = asyncio.run(read_world_envelope([], OverrideStore(_Stored())))  # type: ignore[arg-type]
    assert empty.districts == () and empty.buildings == ()
