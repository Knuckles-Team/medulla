# MEDULLA-WORLD-VIEW — Research

Questions asked before the design, and what the code and the existing specs showed.

## Where do spatial terms belong?

Searched the vocabulary specs (orchestration-vocabulary, data-platform-vocabulary,
supply-chain-vocabulary, media-and-personal-vocabulary) for spatial classes or
relations. None defines `District`, `Building`, `Room`, `Street` or the relations
`locatedIn`, `position`, `hasInterior`. `federated-query-and-reasoning` names a
world view only as a query subject, not as a vocabulary owner. Conclusion: no
existing spec owns the spatial vocabulary; this spec is the owner, and the
classes register under the shared ontology base beside the existing modules.

## What are the endpoints of the spatial relations?

The core and orchestration vocabularies already model the entities space points
at: agents and staffing (MEDULLA-ORCH-VOCAB-R002), agent definitions and prompt
templates (MEDULLA-ORCH-VOCAB-R800), connector packs via the fleet catalog
(ARBORA-FLEET-R040), and agent memory (MEDULLA-DATAPLAT-VOCAB-R001). The spatial
extension adds only the spatial classes and relations; it reuses those classes
as relation endpoints and creates no parallel identity, memory or
orchestration model.

## Stored facts versus derived placement

Storing the derived placement would let the city drift from the fleet: a
connector added or removed in the catalog would require a write to keep the
layout honest. The design keeps derived placement outside the store (computed
from the server id and group on every read) and stores only manual overrides,
each with a stated reason. A stored override for a connector that left the
catalog is reported stale and never applied.

## What does the consumer receive?

One served method, one tenant, a stable envelope: districts, buildings, rooms,
streets, the three relations, and the active overrides. Recorded sessions can
be replayed against the same envelope without a second registry. The method
joins the engine's existing typed-query envelope; no second wire format is
invented for the city.
