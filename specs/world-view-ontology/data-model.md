# MEDULLA-WORLD-VIEW — Data model

Spatial facts of one tenant, in the durable graph (MEDULLA-WORLD-VIEW-R001, MEDULLA-WORLD-VIEW-R002).

## Classes (extension of the core ontology)

- `District` — one served district; named; buildings sit in it via `locatedIn`.
- `Building` — one connector's building in the district; carries `position`
  (x, z, width, depth) and `locatedIn` its district; points at the connector
  pack facts.
- `Room` — one interior unit of a building; `hasInterior` from its building;
  points at agent, memory or tool-call facts.
- `Street` — one walkable route in the district; `position` as ordered
  vertices.

## Relations and properties

- `locatedIn`: building (or room) to district.
- `position`: x, z coordinates and extent (width, depth) for buildings;
  ordered vertices for streets.
- `hasInterior`: building to room.

## Stored versus derived

- Stored: manual overrides only — connector server id, district, plot and
  position, and a stated reason.
- Derived, never stored: the default placement of every connector, computed
  from the connector catalog (server id and group) on each read.
