# Contract — Overstory's Medulla-backed world layout (OVERSTORY-CITY-R021)

One cross-repo seam: Overstory's `arbora-city` follow-on row consumes this
spec's vocabulary and layout to replace its epistemic-graph placeholder with
Medulla facts; the read travels through arbora's served layout read
(ARBORA-FLEET-R084).

Provider: medulla MEDULLA-WORLD-VIEW-R003 (layout read; MEDULLA-WORLD-VIEW-R001 vocabulary, MEDULLA-WORLD-VIEW-R002 override rules)
Consumer: overstory OVERSTORY-CITY-R021

## Wire shape

- Overstory receives the layout through arbora's served layout read; this
  contract fixes what Medulla must provide behind that read.
- Spatial facts (districts, buildings, rooms, streets; `locatedIn`,
  `position`, `hasInterior`) are expressed in the world-view vocabulary, so
  Overstory's city graph and Medulla's graph share one model of space.
- The read is single-tenant and stable: the same tenant and catalog yield the
  same envelope, so recorded sessions can be replayed.

## Refusal codes

- Unknown tenant or unknown layout: typed refusal, surfaced by arbora to the
  city view; the city keeps its current (simulated) layout and labels it as
  simulated.
- No partial envelopes: a refused read returns no layout data.
