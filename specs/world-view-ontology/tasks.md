# MEDULLA-WORLD-VIEW tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md). Rows: [requirements.md](requirements.md).

Producer first: the vocabulary (R001) lands before the layout derivation (R002), and the served method (R003) last; each row lands with its bound test.

- [ ] T001 [US1] MEDULLA-WORLD-VIEW-R001 Declare the spatial vocabulary (District, Building, Room, Street as `:Place` subclasses; hasInterior, position, positionX, positionZ, extentWidth, extentDepth; core locatedIn reused) in the existing world-view module `worldview-v1.ttl`.
- [ ] T002 [US1] MEDULLA-WORLD-VIEW-R002 Implement the layout derivation over the tenant connector catalog (districts by capability, deterministic plots) and the override store (stored only, with reason; stale reporting for removed connectors).
- [ ] T003 [US1] MEDULLA-WORLD-VIEW-R003 Implement the one served spatial query method (stable envelope, typed refusals, no writes on read).
- [ ] T004 [US1] MEDULLA-WORLD-VIEW-R001 Land the bound vocabulary test from [test-spec.md](test-spec.md) and run the ontology gate checks.
- [ ] T005 [US1] MEDULLA-WORLD-VIEW-R002 Land the bound layout test from [test-spec.md](test-spec.md) (derived placement never stored; override wins; stale override reported).
- [ ] T006 [US1] MEDULLA-WORLD-VIEW-R003 Land the bound served test from [test-spec.md](test-spec.md) (full envelope; typed refusals; no write on read).
