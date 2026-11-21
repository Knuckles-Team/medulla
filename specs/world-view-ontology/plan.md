# MEDULLA-WORLD-VIEW — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The world view is a spatial extension of the core ontology: four spatial
classes (`District`, `Building`, `Room`, `Street`), three spatial relations
(`locatedIn`, `position`, `hasInterior`) and their properties, registered
under the shared ontology base. The city layout of a tenant is stored as
spatial facts: deterministic placement derived from the connector catalog,
plus explicit manual overrides with stated reasons. One served method returns
the layout (stored facts plus the derived placement computed on the read) in
a stable envelope. The live consumer is Overstory's Arbora City follow-on
rows, through arbora's served layout read. No existing module owns spatial
facts; the reuse candidates found by searching the code are the shared
ontology base, the existing vocabulary modules, and the engine's typed-query
envelope.

## Technical Context

- **Language/Version**: Rust (the native engine, workspace edition) with the engine's Python test surface; vocabulary registration is data, not a new crate.
- **Primary Dependencies**: the shared ontology base, the existing vocabulary modules (orchestration vocabulary for agents and definitions, data-platform vocabulary for memory), and the engine's typed-query envelope. No new crates.
- **Storage**: tenant-scoped graph facts committed through the existing mutation path; overrides are facts, derived placement is not persisted.
- **Testing**: Python bound tests through the engine's Python client (`pytest`); the three planned tests are named in [test-spec.md](test-spec.md), one per row.
- **Target Platform**: the native Rust engine, single-node build; Linux development environment.
- **Project Type**: library/engine (native Rust engine with its Python client and bindings).
- **Performance Goals**: the served method reads at most one district layout, bounded by the connector catalog size of the tenant; no benchmark row until the method lands — it joins the existing typed-query benchmark lane then.
- **Constraints**: no new crates, no served authority, no second ontology; vocabulary registration is data; a read never writes.
- **Scale/Scope**: one district, at most a few hundred connectors per tenant, single-tenant reads.

## Constitution Check

- Single owner: pass — Medulla owns the vocabulary, the facts and the query; the served operation and the UI belong to arbora and overstory.
- No parallel model: pass — the extension registers under the shared ontology base and points at existing entity classes.
- Reuse before adding: pass — the typed-query envelope, the mutation path and the ontology gate are reused; no new crate or store.
- Evidence over claims: pass — each row names a bound test; the gate checks run before landing.

## Project Structure

- `crates/` — the world-view vocabulary module and layout derivation beside
  the existing vocabulary definitions (exact module named at implementation).
- `tests/test_world_view_ontology.py`, `tests/test_world_view_layout.py`,
  `tests/test_world_view_served.py` — the bound tests (planned names).

## Phase 0: Research

- Where do the spatial classes register? The shared ontology base hosts the
  existing vocabulary modules (orchestration, data-platform, supply-chain);
  the world view joins beside them, not as a parallel model (review finding 14).
- What is stored and what is derived? The review direction (finding 15):
  derived placement is computed from the connector catalog and never stored;
  only manual overrides are stored, each with a stated reason, so the city
  cannot drift from the fleet.
- What does the consumer receive? A stable envelope for one tenant layout,
  answered from stored facts plus the derived placement computed on the read.

See [research.md](research.md) for the questions and the evidence found.

## Phase 1: Design

- **Owner**: the existing world-view module
  `crates/medulla-core/ontology/worldview-v1.ttl` (core spec `worldview`)
  holds the spatial vocabulary; the layout derivation and served method sit
  beside the engine's existing vocabulary handling.
- **Cohesion**: one domain (city spatial facts) in one module: vocabulary,
  layout derivation, override store and the served method together.
- **Dependency direction**: the world-view module depends on the core
  ontology classes (agents, definitions, prompts, connectors, memory);
  consumers (arbora's read, overstory's city) depend on this module; nothing
  in the engine depends back on the world view.
- **Interface depth**: one vocabulary module, one layout derivation, one
  served method; no additional surface types are introduced; reads never
  write.
- **Special cases**: an empty catalog derives an empty layout; an override for
  a removed connector is reported stale; unknown tenant or unknown layout is a
  typed refusal, never a partial answer.
- **State**: stored facts are tenant-scoped overrides only; the derived
  placement is recomputed on every read; no persisted state beyond overrides.
- **Environment**: single-node engine build; the test harness seeds tenant
  facts through the existing mutation path and calls the served method
  through the typed-query envelope.

## Phase 2: Task approach

- R001 first (vocabulary), then R002 (derivation + override store), then R003
  (the served method). Each lands with its bound test.

## Complexity Tracking

New surface is one vocabulary module, one derivation and one method. Budget:
the served method reads at most one district layout (bounded by the connector
catalog); no benchmark row yet — the method joins the existing typed-query
benchmark lane when it lands.

## Build contract

The vocabulary registers without a schema migration (ontology extension),
the override store commits through the existing mutation path, and the served
method reuses the typed-query envelope. CI runs the three bound tests; no
release-bundle change until the served method lands.

## Production caller chain

(Planned) Overstory's Arbora City renders the layout it receives from
arbora's served layout read; arbora's read (ARBORA-FLEET-R084) invokes this
served spatial query as the bound actor. Until arbora's read lands, the
consumer is the engine's own served test.

## Cross-repo seams

- [ARBORA-FLEET-R084](https://github.com/Knuckles-Team/arbora/blob/main/specs/fleet-catalog-and-tools/requirements.md) — arbora serves the city layout read from these spatial facts (provider: this spec R003; consumer: arbora).
- [OVERSTORY-CITY-R021](https://github.com/Knuckles-Team/overstory/blob/main/specs/arbora-city/requirements.md) — Overstory's Medulla-backed world layout follow-on consumes this vocabulary and layout (consumer: overstory; provider: this spec R001/R002/R003).

### Amendments

2026-11-16 — Initial plan: spatial vocabulary as an extension of the core
ontology (not a parallel model), derived placement with stored overrides,
one served spatial query method. Cross-repo seams to arbora's layout read
and Overstory's world-layout follow-on named.
