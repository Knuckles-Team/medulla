# Seam: object storage placement route

Provider: medulla MEDULLA-PLACEMENT-R611
Consumer: medulla MEDULLA-OBJECT-STORAGE-BACKEND-R004

## Wire shape

The route is the engine's `PlacementRouteRequest` (owner MEDULLA-ENGINE-PARITY) in `crates/medulla-result-types/src/epistemic_operations/placement_operations.rs`. The tenant client (`medulla/placement.py`) holds the bounded per-tenant pool and the breaker; it asks the engine for the member list and the full route and rejects stale or divergent snapshots. A node opens a tenant graph from the shared root only with a placement route.

## Refusal codes

`PLACEMENT_STALE` (a snapshot older than the routing epoch), `PLACEMENT_NOT_GRANTED` (a node without a route refuses before fetching any segment).
