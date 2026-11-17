# MEDULLA-WORLD-VIEW — World-view spatial ontology and facts

Owner: Medulla. Cross-repo IDs: ARBORA-FLEET-R084, OVERSTORY-CITY-R021.
Public provenance: spec-only change; files the spatial vocabulary and facts the served city view (Overstory's Arbora City) needs, per the arbora-city review points 14–15.

## User Scenarios & Testing

### User Story 1 - Spatial facts for the served city view (Priority: P1)

As the consumer of a served city view (Overstory's Arbora City, through arbora's served layout read), I need spatial facts — districts, buildings, rooms and streets, and how they relate — so the city layout can be rendered and replayed from Medulla facts instead of a second registry or a client-side guess.

**Acceptance Scenarios**:

1. **Given** the world-view module is registered under the shared ontology base, **When** the ontology is loaded, **Then** `District`, `Building`, `Room` and `Street` exist as classes and `locatedIn`, `position` and `hasInterior` exist as relations, and no parallel identity, memory or orchestration model exists.
2. **Given** a tenant's connector catalog facts, **When** the layout is derived, **Then** every connector gets a deterministic district and plot with no stored derived fact; a stored manual override with a stated reason supersedes the derived placement; an override for a connector removed from the catalog is reported stale, never applied.
3. **Given** a seeded tenant layout, **When** the one served spatial query method is called, **Then** the stable envelope (districts, buildings, rooms, streets, the three relations, active overrides) is served from stored facts plus the derived placement computed on the read, and an unknown tenant or an unknown layout refuses with a typed reason.

### Edge Cases

- A catalog with zero connectors derives an empty layout (no district), not an error.
- A stored override for a connector absent from the catalog is reported stale and excluded from the served layout.
- A second tenant's layout is never visible to the first; tenant isolation holds on the served method.

## Requirements

- **FR-001**: the world-view spatial vocabulary extends the core ontology under the shared ontology base; it adds no parallel model. Rows: `MEDULLA-WORLD-VIEW-R001`.
- **FR-002**: the city layout is derived from the connector catalog; only manual overrides are stored, each with a stated reason. Rows: `MEDULLA-WORLD-VIEW-R002`.
- **FR-003**: spatial facts are served through one method with a stable envelope and typed refusals; a read never writes. Rows: `MEDULLA-WORLD-VIEW-R003`.

## Success Criteria

- **SC-001**: the three bound tests pass and the ontology gate checks stay green (one module per spatial domain, no duplicate authority). Rows: `MEDULLA-WORLD-VIEW-R001`, `MEDULLA-WORLD-VIEW-R002`, `MEDULLA-WORLD-VIEW-R003`.

## Assumptions

- The core and orchestration vocabularies already model the entities the spatial relations point at (agents, agent definitions, prompt templates, workflow patterns, connector packs, agent memory); the spatial extension adds only space.
- The served city is one district of at most a few hundred connectors; the layout read is single-tenant.
- Medulla does not own the served operation or the UI: the served layout read is arbora's seam, the city view is overstory's.

## Why a new spec

Searched the existing specs: the vocabulary specs (orchestration-vocabulary, data-platform-vocabulary, supply-chain-vocabulary, media-and-personal-vocabulary) define domain vocabularies, but none defines spatial classes or relations, and no spec owns a city layout as spatial facts. `federated-query-and-reasoning` names a world view only as a query subject. No existing spec owns this capability; this spec is the owner.

## Scope and interfaces

Owned: the four spatial classes, the three spatial relations and their properties, the layout derivation, the override store, and the one served spatial query method. Cited, not re-specified: the fleet catalog (ARBORA-FLEET-R040 in the arbora repo), the served layout read (ARBORA-FLEET-R084, seam below), the consumer city view (OVERSTORY-CITY-R021, seam below). Non-goals: rendering, simulation, served operations and admission (arbora), the city UI (overstory).

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-WORLD-VIEW-R001` | plan.md Phase 1: Design | `tests/test_world_view_ontology.py` | PENDING |
| FR-002 | `MEDULLA-WORLD-VIEW-R002` | plan.md Phase 1: Design | `tests/test_world_view_layout.py` | PENDING |
| FR-003 | `MEDULLA-WORLD-VIEW-R003` | plan.md Phase 1: Design | `tests/test_world_view_served.py` | PENDING |
