# Contract — arbora's served city layout read (ARBORA-FLEET-R084)

One cross-repo seam: the arbora spec `fleet-catalog-and-tools` serves the city
layout read; this spec provides the spatial facts and the spatial query behind
it.

Provider: medulla MEDULLA-WORLD-VIEW-R003
Consumer: arbora ARBORA-FLEET-R084

## Wire shape

- Request: one tenant id (the fleet tenant), nothing else; on the Medulla side
  the read is the one served spatial query method.
- Response envelope (stable): `districts` (id, name), `buildings` (connector
  server id, district id, x, z, width, depth), `rooms` (building id, room id,
  kind), `streets` (id, ordered vertices), `relations` (`locatedIn`,
  `position`, `hasInterior` triples), and `overrides` (connector server id,
  district, position, stated reason).
- The envelope is renderable and replayable: recorded sessions replay against
  it without a second registry.
- Arbora invokes the read as the bound actor through its served operation
  path; Medulla never serves the city UI and takes no browser traffic.

## Refusal codes

- Unknown tenant: typed refusal, no partial answer.
- Unknown layout (the tenant has no connector catalog facts): typed refusal.
- A read never writes; any write attempt is a defect refused at the mutation
  gate.
