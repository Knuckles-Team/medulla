# MEDULLA-WORLD-VIEW test spec

Governing spec: [spec.md](spec.md).

## Positive tests

- `tests/test_world_view_ontology.py::test_spatial_classes_extend_the_core_ontology`
  (planned) — registers the vocabulary module and asserts: `District`,
  `Building`, `Room`, `Street` exist as classes; `locatedIn`, `position`,
  `hasInterior` exist as relations; each is registered under the shared
  ontology base; no identity, memory or orchestration class is introduced by
  the module; the module declares one IRI module. Bound:
  `MEDULLA-WORLD-VIEW-R001`.
- `tests/test_world_view_layout.py::test_derived_placement_is_never_stored`
  (planned) — derives the layout for a tenant catalog (asserts districts
  grouped by capability and deterministic plot positions); adds one connector
  and asserts the derived placement changed while the stored facts contain
  only overrides; stores one override and asserts it supersedes the derived
  placement; removes the connector and asserts the override is reported
  stale, never applied. Bound: `MEDULLA-WORLD-VIEW-R002`.
- `tests/test_world_view_served.py::test_spatial_query_serves_the_layout_and_refuses_unknown_tenants`
  (planned) — serves a seeded tenant layout through the method and asserts
  the full envelope (districts, buildings, rooms, streets, `locatedIn`,
  `position`, `hasInterior`, active overrides); asserts the read performed
  no write; asserts an unknown tenant and an unknown layout each refuse with
  a typed reason. Bound: `MEDULLA-WORLD-VIEW-R003`.

## Negative and boundary cases

- A catalog with zero connectors derives an empty layout (no district), not
  an error (R002).
- An override for a connector absent from the catalog is reported stale and
  excluded from the served layout (R002/R003).
- A second tenant's layout is never visible to the first (R003).

## Quality and release proof

- The vocabulary module keeps the existing ontology-gate checks green
  (no duplicate authority, one module per domain).
- No benchmark row yet: the served method joins the engine's typed-query
  benchmark lane when it lands; the method is bounded by one district layout.
