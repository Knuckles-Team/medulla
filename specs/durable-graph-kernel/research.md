# Research: durable graph kernel decisions

## RESP listener enters the served KV methods (MEDULLA-DURABLE-KERNEL-R860.3)

Decision: the RESP listener keeps no store. Every served string command enters the
dispatch through one verified request context built after `AUTH`
(`VerifiedRequestContext::authenticated_resp_actor`, grant `kv:read` and `kv:write`
only, never `kg:*` or `broker:*`), and reaches `KvGet`, `KvPut`, `KvDelete` and `KvCas`.
The principal's private KV scope is the keyspace, so isolation is the engine's own.

Why: one admission and policy path for every wire; one durable store; no second
`kv.redb` with its own physical identity.

Consequences, decided from the north star (one engine, no sync; hard cutover):
- `MULTI`/`EXEC`/`DISCARD` are refused. The served methods commit one key at a time, so
  an atomic multi-command transaction would be a false promise.
- `SET` options (`EX`, `PX`, `NX`, `XX`) are refused; expiry and conditional writes
  have no served KV method.
- `DECR`, `INCRBY`, `MSET`, `SCAN` and `TYPE` are refused rather than half-served.
- The `INCR` retry bound is the registry knob `resp.incr_retry_limit`.
- Request ids start from the process start time in microseconds so the durable mutation
  ledger never sees a repeated idempotency key after a restart.

## Graph change feed over the one CDC feed (R043.100.2, R043.100.3)

- Decision: `OpenGraphSnapshot` and `ReadGraphChanges` read the change feed `CdcHub` already keeps
  for every committed graph write. A second ledger or store would be a second write path and two
  orders of the same commits.
- Decision: a feed's incarnation is minted when the feed is created and again when the graph is
  cleared. A restart creates new feeds. A cursor of an earlier incarnation is stale, never
  silently re-read from the same numbers.
- Decision: the authorization revision is a digest of the agent and the sorted scopes of the
  verified token. No separate revision counter exists; a grant or revocation changes the token
  scopes, so the digest is the revision.
- Decision: an event is withheld when none of its images passes the caller's row-level predicate.
  The page cursor advances past withheld events, so a page never reveals how many were withheld.
- Retention is the retained ring window of the hub; a cursor behind it is `CURSOR_EXPIRED`.
## S3 objects are blobs the native blob methods read (MEDULLA-DURABLE-KERNEL-R860.6)

Decision: the S3 listener stores each object as a blob manifest (content-defined chunks plus
manifest) whose owner scope is the carrier scope of its fixed service principal
`service:s3-api-client`. The object digest and the ETag are the manifest digest. The listener
keeps its bucket and object listing index in its own durable KV store.

Why: the hot-path proof reads an S3 object back through `BlobFetchBegin` and `BlobChunkGet`,
which serve only a manifest owned by the caller's scope. A raw chunk digest with no manifest
was not readable through the served blob methods, so the S3 surface was a second store in
practice. Owning the manifest by the service principal keeps one blob authority with one
ownership rule and needs no new method.

Consequences: an ETag is a manifest digest, not the digest of the bytes; identical bytes still
share one digest. A multipart upload keeps its parts as staged chunks and writes one manifest
at completion.


## Concept ids and lineage edges are tenant-keyed Agent Library rows (MEDULLA-DURABLE-KERNEL-R094.7, R094.7.1)

Decision: `concept_ids` is keyed `(tenant, concept id)` and `concept_lineage_edges` is keyed
`(tenant, superseded, successor)`, both in the Agent Library owner. The served method is
`ConceptLineage` with ops `claim`, `add_edge` and `read`; writes commit through the admitted
pack-write lifecycle (replay identity, staged batch, owner rows, receipt), reads come from one
committed snapshot.

Why: a principal or repository name can exist in two tenants, and a key without the tenant would
let one tenant's claim collide with, or make visible, another's (ISO-OWNER-KEY). The owner already
carries the receipt, replay and outbox rows a durable write needs, so no second store is added.

Decisions made while building:
- The decision to allow a claim or an edge stays in `ConceptPathSet::claim` and
  `ConceptLineage::add_edge`. The adapter loads only the rows those calls need: the two endpoint
  rows, and the edges reachable from the successor (a cycle through a new edge `a -> b` is a path
  `b -> ... -> a`), so the write costs the number of edges touched, as the row requires.
  `ConceptLineage::restore_edge` puts stored edges back without re-checking them; the store only
  ever holds edges `add_edge` accepted.
- Ids travel as text and are parsed by the handler, so a malformed id refuses `CONCEPT_ID_MALFORMED`
  and a path claimed by another repository refuses `CONCEPT_ID_PATH_COLLISION` (the landed
  `ConceptIdError`, given codes), instead of failing the wire decode with an untyped error.
- Authorization reuses `agent:component-write` and `agent:component-read`: the concept store is part
  of the component catalog and no new privilege is introduced.
- `Read` returns the concept and at most `MAX_CONCEPT_SUCCESSORS_READ` direct successors with a
  `truncated` flag, so a page is never mistaken for the whole set.
- Re-claiming an id by its owner, and re-adding an edge, return the first stored evidence and write
  nothing new.
- LAYOUT-REGEN needed: the Agent Library layout digest pin in `owner/lineage.rs` is replaced by the
  compiled digest (the foundation commit already declares the predecessor and upgrade).
- WIRE-REGEN needed: `ConceptLineage` is a new method; the wire vectors are refreshed by the gate.
