# MEDULLA-WORLD-VIEW test spec

Governing spec: [spec.md](spec.md).

## Positive tests

- `tests/test_world_view_ontology.py::test_spatial_classes_extend_the_core_ontology`
  — loads the packaged world-view module (`worldview-v1.ttl`) and asserts:
  the module IRI is declared; `District`, `Building`, `Room`, `Street` are
  classes under the core namespace whose only parent is the core `Place`;
  `hasInterior` and `position` are declared; `locatedIn` is the core relation
  and is not redeclared; no identity, memory, orchestration or store class is
  introduced. Bound: `MEDULLA-WORLD-VIEW-R001`.
- `tests/test_world_view_layout.py::test_derived_placement_is_never_stored`
  (planned) — derives the layout for a tenant catalog (asserts districts
  grouped by capability and deterministic plot positions); adds one connector
  and asserts the derived placement changed while the stored facts contain
  only overrides; stores one override and asserts it supersedes the derived
  placement; removes the connector and asserts the override is reported
  stale, never applied. Bound: `MEDULLA-WORLD-VIEW-R002`.
- `tests/test_world_view_layout.py::test_world_envelope_is_derived_overridden_and_read_only`
  (planned) - reads the layout envelope (districts, buildings in placement
  order, `overridden` and `reason`, stale overrides) over the in-memory fake
  client, asserts an empty catalog gives the empty envelope and that the read
  sent no write. Bound: `MEDULLA-WORLD-VIEW-R003`.

## Negative and boundary cases

- A catalog with zero connectors derives an empty layout (no district), not
  an error (R002).
- An override for a connector absent from the catalog is reported stale and
  excluded from the served layout (R002/R003).
- A second tenant's layout is never visible to the first: the tenant comes from the connection, so the envelope read carries no tenant parameter (R003).

## Quality and release proof

- The vocabulary module keeps the existing ontology-gate checks green
  (no duplicate authority, one module per domain).
- No benchmark row yet: the served method joins the engine's typed-query
  benchmark lane when it lands; the method is bounded by one district layout.

- `MEDULLA-WORLD-VIEW-R004`: `tests/test_world_view_city_state.py::test_snapshot_is_revisioned_and_heartbeat_is_subject_bound_and_fenced` (L2) (planned): proves a stale revision refuses `REVISION_CONFLICT`; another subject's write refuses; an expired human node is absent from the snapshot.
