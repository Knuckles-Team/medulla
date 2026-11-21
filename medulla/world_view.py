"""Derived world layout with stored manual overrides (MEDULLA-WORLD-VIEW-R002).

The placement of each connector is a pure function of the connector catalog and is
never stored. Only a manual override is stored, as a `:Building` node with a stated
reason. `derive_placements` and `apply_overrides` perform no I/O; `OverrideStore` is
the only writer.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any

import msgpack

from .generated import graph as _graph

PLOT_COLUMNS = 8
PLOT_PITCH = 20
DISTRICT_PITCH = 200
UNGROUPED_DISTRICT = "ungrouped"
OVERRIDE_ID_PREFIX = "world:override:"
OVERRIDE_READ_LIMIT = 1000


@dataclass(frozen=True)
class CatalogEntry:
    """One connector of the catalog: its server id and its group."""

    server_id: str
    group: str


@dataclass(frozen=True)
class Placement:
    """The derived position of one connector."""

    server_id: str
    district: str
    plot: int
    x: int
    z: int


@dataclass(frozen=True)
class Override:
    """A manual position for one connector, with the reason it was moved."""

    server_id: str
    x: int
    z: int
    reason: str

    def __post_init__(self) -> None:
        if not self.reason.strip():
            raise ValueError("WORLD_VIEW_REASON_REQUIRED")


@dataclass(frozen=True)
class Layout:
    """The derived placements with the overrides applied, and the overrides that
    name a connector the catalog no longer holds."""

    placements: tuple[Placement, ...]
    stale_overrides: tuple[str, ...]


def _byte_order(text: str) -> bytes:
    return text.encode("utf-8")


def derive_placements(catalog: Sequence[CatalogEntry]) -> tuple[Placement, ...]:
    """The deterministic placement of every catalog entry; the input order does not
    matter."""
    seen: set[str] = set()
    for entry in catalog:
        if entry.server_id in seen:
            raise ValueError(f"WORLD_VIEW_DUPLICATE_SERVER: {entry.server_id}")
        seen.add(entry.server_id)
    by_district: dict[str, list[str]] = {}
    for entry in catalog:
        district = entry.group or UNGROUPED_DISTRICT
        by_district.setdefault(district, []).append(entry.server_id)
    placements: list[Placement] = []
    for index, district in enumerate(sorted(by_district, key=_byte_order)):
        for plot, server_id in enumerate(sorted(by_district[district], key=_byte_order)):
            placements.append(
                Placement(
                    server_id=server_id,
                    district=district,
                    plot=plot,
                    x=index * DISTRICT_PITCH + (plot % PLOT_COLUMNS) * PLOT_PITCH,
                    z=(plot // PLOT_COLUMNS) * PLOT_PITCH,
                )
            )
    return tuple(placements)


def apply_overrides(
    derived: Sequence[Placement], overrides: Sequence[Override]
) -> Layout:
    """Replace `x` and `z` of each overridden placement; an override for a connector
    that is not derived is listed in `stale_overrides` (sorted) and never applied."""
    by_server = {override.server_id: override for override in overrides}
    known = {placement.server_id for placement in derived}
    placed = tuple(
        Placement(
            server_id=placement.server_id,
            district=placement.district,
            plot=placement.plot,
            x=by_server[placement.server_id].x,
            z=by_server[placement.server_id].z,
        )
        if placement.server_id in by_server
        else placement
        for placement in derived
    )
    stale = tuple(sorted((sid for sid in by_server if sid not in known), key=_byte_order))
    return Layout(placements=placed, stale_overrides=stale)


def _properties(raw: Any) -> dict[str, Any]:
    if isinstance(raw, (bytes, bytearray)):
        decoded = msgpack.unpackb(bytes(raw), raw=False)
        return decoded if isinstance(decoded, dict) else {}
    return raw if isinstance(raw, dict) else {}


class OverrideStore:
    """The stored manual overrides of one tenant, one `:Building` node each. The
    tenant and principal come from the connection."""

    def __init__(self, client: Any) -> None:
        self._client = client

    async def put(self, override: Override) -> None:
        properties = {
            "type": "Building",
            "server_id": override.server_id,
            "position_x": override.x,
            "position_z": override.z,
            "override_reason": override.reason,
        }
        await _graph.send_add_node(
            self._client,
            {
                "node_id": f"{OVERRIDE_ID_PREFIX}{override.server_id}",
                "properties_msgpack": msgpack.packb(properties, use_bin_type=True),
            },
        )

    async def clear(self, server_id: str) -> None:
        await _graph.send_remove_node(
            self._client, {"node_id": f"{OVERRIDE_ID_PREFIX}{server_id}"}
        )

    async def list(self) -> tuple[Override, ...]:
        rows = await _graph.send_get_nodes_by_label(
            self._client, {"label": "Building", "limit": OVERRIDE_READ_LIMIT}
        )
        found = []
        for node_id, raw in rows:
            if not str(node_id).startswith(OVERRIDE_ID_PREFIX):
                continue
            properties = _properties(raw)
            found.append(
                Override(
                    server_id=properties["server_id"],
                    x=int(properties["position_x"]),
                    z=int(properties["position_z"]),
                    reason=properties["override_reason"],
                )
            )
        return tuple(sorted(found, key=lambda item: _byte_order(item.server_id)))
