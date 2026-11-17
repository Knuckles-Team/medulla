# MEDULLA-WORLD-VIEW — Quickstart

How to run the proof of the served behaviour (MEDULLA-WORLD-VIEW-R003) once
the served method lands (planned names, per [test-spec.md](test-spec.md)):

```bash
pytest tests/test_world_view_served.py::test_spatial_query_serves_the_layout_and_refuses_unknown_tenants
```

Scenario: seed one tenant's connector catalog facts, store one manual override
with a stated reason, call the served spatial query, and assert the stable
envelope (districts, buildings, rooms, streets, `locatedIn`, `position`,
`hasInterior`, the active override) plus the typed refusals for an unknown
tenant and for an unknown layout, with no write on the read.
