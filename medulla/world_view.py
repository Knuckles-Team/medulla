"""Derived world layout with stored manual overrides (MEDULLA-WORLD-VIEW-R002), the
world envelope read from it (MEDULLA-WORLD-VIEW-R003) and the revisioned city state
(MEDULLA-WORLD-VIEW-R004).

The placement of each connector is a pure function of the connector catalog and is
never stored. Only a manual override is stored, as a `:Building` node with a stated
reason. `derive_placements` and `apply_overrides` perform no I/O; `OverrideStore` is
the only writer.
"""

from __future__ import annotations

import hashlib
import re
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
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
        for plot, server_id in enumerate(
            sorted(by_district[district], key=_byte_order)
        ):
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
    stale = tuple(
        sorted((sid for sid in by_server if sid not in known), key=_byte_order)
    )
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


WORLD_ENVELOPE_VERSION = 1


@dataclass(frozen=True)
class WorldDistrict:
    """One district of the world and its position in the district order."""

    name: str
    index: int


@dataclass(frozen=True)
class WorldBuilding:
    """One connector building: where it stands and, when a manual override moved it,
    why."""

    server_id: str
    district: str
    plot: int
    x: int
    z: int
    overridden: bool
    reason: str | None


@dataclass(frozen=True)
class WorldEnvelope:
    """The world a caller draws: districts, buildings and the stale overrides. The
    wire shape is `dataclasses.asdict` of this value, tuples as JSON arrays."""

    version: int
    districts: tuple[WorldDistrict, ...]
    buildings: tuple[WorldBuilding, ...]
    stale_overrides: tuple[str, ...]


def build_world_envelope(
    layout: Layout, overrides: Sequence[Override]
) -> WorldEnvelope:
    """The envelope of a layout. Districts are the distinct placement districts in byte
    order; buildings keep the layout order with the overridden coordinates; a building
    is `overridden` with its reason when an applied override named it. Pure."""
    districts = tuple(
        WorldDistrict(name=name, index=index)
        for index, name in enumerate(
            sorted(
                {placement.district for placement in layout.placements}, key=_byte_order
            )
        )
    )
    stale = set(layout.stale_overrides)
    reasons = {
        override.server_id: override.reason
        for override in overrides
        if override.server_id not in stale
    }
    buildings = tuple(
        WorldBuilding(
            server_id=placement.server_id,
            district=placement.district,
            plot=placement.plot,
            x=placement.x,
            z=placement.z,
            overridden=placement.server_id in reasons,
            reason=reasons.get(placement.server_id),
        )
        for placement in layout.placements
    )
    return WorldEnvelope(
        version=WORLD_ENVELOPE_VERSION,
        districts=districts,
        buildings=buildings,
        stale_overrides=layout.stale_overrides,
    )


async def read_world_envelope(
    catalog: Sequence[CatalogEntry], store: OverrideStore
) -> WorldEnvelope:
    """Derive the placements, read the stored overrides, apply them and build the
    envelope. It performs no write; the tenant comes from the connection."""
    derived = derive_placements(catalog)
    overrides = await store.list()
    return build_world_envelope(apply_overrides(derived, overrides), overrides)


PRESENCE_LABEL = "CityPresence"
PRESENCE_ID_PREFIX = "world:presence:"
PRESENCE_READ_LIMIT = 10000
PRESENCE_KINDS = ("npc", "ai", "human")
ACTIVITY_STATES = ("walking", "driving", "flying", "sailing", "inside")
TARGET_REF_PATTERN = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")
HEARTBEAT_TTL = timedelta(seconds=60)


class CityStateRefused(ValueError):
    """A city-state call the engine contract refuses. `code` is the refusal code
    (`INVALID_ARGUMENT`, `REVISION_CONFLICT` or `SUBJECT_ACCESS_DENIED`); the message
    is `<code>: <detail>`."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code


@dataclass(frozen=True)
class CityEntity:
    """One presence in the city: an `npc`, an `ai` or a `human`, where it is, what it is
    doing and the revision of its node."""

    node_id: str
    presence_kind: str
    subject: str | None
    x: float
    y: float
    heading: float
    activity_state: str | None
    target_ref: str | None
    observed_at: str | None
    expires_at: str | None
    revision: int


@dataclass(frozen=True)
class CityStateSnapshot:
    """The city state a caller draws. `revision` is the highest entity revision (0 for
    an empty city), `simulated_at` the instant the snapshot answers for and
    `layout_revision` a number that changes whenever the stored layout overrides do."""

    revision: int
    simulated_at: str
    layout_revision: int
    entities: tuple[CityEntity, ...]


def _utc_now() -> datetime:
    return datetime.now(UTC)


def _instant(value: datetime) -> str:
    return value.astimezone(UTC).isoformat()


def layout_revision_of(overrides: Sequence[Override]) -> int:
    """The layout revision of a set of stored overrides: the integer of the first eight
    bytes of the SHA-256 of their canonical listing, and 0 for none."""
    if not overrides:
        return 0
    listing = "\n".join(
        f"{item.server_id}\t{item.x}\t{item.z}\t{item.reason}"
        for item in sorted(overrides, key=lambda item: _byte_order(item.server_id))
    )
    return int.from_bytes(hashlib.sha256(listing.encode()).digest()[:8], "big")


def _entity(node_id: str, properties: dict[str, Any]) -> CityEntity:
    return CityEntity(
        node_id=node_id,
        presence_kind=properties["presence_kind"],
        subject=properties.get("presence_subject"),
        x=float(properties["world_x"]),
        y=float(properties["world_y"]),
        heading=float(properties.get("heading", 0)),
        activity_state=properties.get("activity_state"),
        target_ref=properties.get("target_ref"),
        observed_at=properties.get("observed_at"),
        expires_at=properties.get("expires_at"),
        revision=int(properties["revision"]),
    )


class CityStateReader:
    """The read of the city state over `GetNodesByLabel`; it performs no write. The
    tenant comes from the connection. `overrides` names the store whose stored layout
    overrides the snapshot's `layout_revision` follows; `clock` is the time source."""

    def __init__(
        self,
        client: Any,
        overrides: OverrideStore | None = None,
        clock: Callable[[], datetime] = _utc_now,
    ) -> None:
        self._client = client
        self._overrides = overrides
        self._clock = clock

    async def snapshot(self, tenant: str) -> CityStateSnapshot:
        """The tenant's presences at the clock's instant. An expired `human` node is
        absent; entities are in ascending node-id byte order."""
        del tenant  # the connection names the tenant; the argument documents the call
        now = self._clock()
        rows = await _graph.send_get_nodes_by_label(
            self._client, {"label": PRESENCE_LABEL, "limit": PRESENCE_READ_LIMIT}
        )
        found = []
        for node_id, raw in rows:
            if not str(node_id).startswith(PRESENCE_ID_PREFIX):
                continue
            entity = _entity(str(node_id), _properties(raw))
            if (
                entity.presence_kind == "human"
                and entity.expires_at is not None
                and datetime.fromisoformat(entity.expires_at) <= now
            ):
                continue
            found.append(entity)
        found.sort(key=lambda item: _byte_order(item.node_id))
        overrides = await self._overrides.list() if self._overrides else ()
        return CityStateSnapshot(
            revision=max((entity.revision for entity in found), default=0),
            simulated_at=_instant(now),
            layout_revision=layout_revision_of(overrides),
            entities=tuple(found),
        )


class CityStateWriter:
    """The one writer a person's presence has: `heartbeat` writes the subject's own
    `human` node. `principal` is the verified principal of the connection. `ai` and
    `npc` nodes are not written here; they change only from recorded run transitions."""

    def __init__(
        self,
        client: Any,
        principal: str,
        clock: Callable[[], datetime] = _utc_now,
    ) -> None:
        self._client = client
        self._principal = principal
        self._clock = clock

    async def heartbeat(
        self,
        subject: str,
        expected_revision: int,
        x: float,
        y: float,
        heading: float,
        activity_state: str,
        target_ref: str | None,
    ) -> int:
        """Write the subject's `human` node at `expected_revision + 1` and return the
        new revision. Revision 0 creates the node. A subject that is not the principal
        refuses `SUBJECT_ACCESS_DENIED`; an `activity_state` outside `ACTIVITY_STATES`
        or a `target_ref` outside `TARGET_REF_PATTERN` refuses `INVALID_ARGUMENT`, both
        before any write; a node whose revision is not `expected_revision` refuses
        `REVISION_CONFLICT`."""
        if subject != self._principal:
            raise CityStateRefused(
                "SUBJECT_ACCESS_DENIED", "a heartbeat writes the caller's own node"
            )
        if activity_state not in ACTIVITY_STATES:
            raise CityStateRefused(
                "INVALID_ARGUMENT",
                f"activity_state is one of {', '.join(ACTIVITY_STATES)}",
            )
        if target_ref is not None and not TARGET_REF_PATTERN.fullmatch(target_ref):
            raise CityStateRefused(
                "INVALID_ARGUMENT", "target_ref is 1 to 128 of A-Z a-z 0-9 . _ : -"
            )
        now = self._clock()
        revision = expected_revision + 1
        fields: dict[str, Any] = {
            "world_x": x,
            "world_y": y,
            "heading": heading,
            "activity_state": activity_state,
            # The compare-and-set merges fields, so an absent target is stored as null,
            # which the snapshot reads as absent.
            "target_ref": target_ref,
            "observed_at": _instant(now),
            "expires_at": _instant(now + HEARTBEAT_TTL),
            "revision": revision,
        }
        node_id = f"{PRESENCE_ID_PREFIX}human:{subject}"
        if expected_revision == 0:
            created = await _graph.send_create_node_if_absent(
                self._client,
                {
                    "node_id": node_id,
                    "properties_msgpack": msgpack.packb(
                        {
                            "type": PRESENCE_LABEL,
                            "presence_kind": "human",
                            "presence_subject": subject,
                            **fields,
                        },
                        use_bin_type=True,
                    ),
                },
            )
            written = bool(created)
        else:
            written = await _graph.send_compare_and_set_node_fields(
                self._client,
                {
                    "node_id": node_id,
                    "conditions_msgpack": msgpack.packb(
                        {"revision": expected_revision}, use_bin_type=True
                    ),
                    "updates_msgpack": msgpack.packb(fields, use_bin_type=True),
                },
            )
        if not written:
            raise CityStateRefused(
                "REVISION_CONFLICT",
                f"the node of {subject} is not at revision {expected_revision}",
            )
        return revision
