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
    apply_overrides,
    derive_placements,
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
