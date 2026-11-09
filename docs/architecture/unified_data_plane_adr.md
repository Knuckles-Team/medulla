# ADR: Attached sources are the default; Medulla stays the one converged engine

**Status:** Accepted · **Requirement:** `MEDULLA-UNIFIED-DATA-PLANE-R001` (the
separate pgrx go/no-go decision is `MEDULLA-UNIFIED-DATA-PLANE-R025`)

## Context

[`specs/unified-data-plane/spec.md`](../../specs/unified-data-plane/spec.md) and
[`plan.md`](../../specs/unified-data-plane/plan.md) already state the load-bearing
architecture decisions in prose, scattered across "Outcome and boundaries," the
query contract and the native-path dependency section. `MEDULLA-UNIFIED-DATA-PLANE-R001`
asks that these be recorded as reviewed architecture decision records, not that they
be decided afresh. This ADR is that record: each decision below quotes the merged
spec/plan text it formalizes, so review can confirm the record against its source
rather than against a new unreviewed claim.

## Decision 1 — Attached sources are the default integration path

Third-party applications keep their own database. Medulla does not take over an
application's primary store by default; it attaches to it, and reads the attached
data as live SQL and named virtual graphs.

> "Third-party applications keep their real database. Medulla attaches to each
> through a single owner-scoped registry, serves live SQL and named virtual graphs,
> optionally maintains a change-captured accelerated copy, and exposes a
> freshness-aware query route." — spec.md, "Outcome and boundaries"

**Consequence:** every new application integration starts as an attached source
(registry + OBDA/SQL federation + optional CDC), never as a native-storage request.
Native hosting is a separate, later-earned track (Decision 3), not an alternative
starting point.

## Decision 2 — Medulla remains the one converged query engine

Medulla is the one query surface across every modality for Medulla-owned data;
attaching a source does not fork a second query engine or a parallel storage authority.

> "Medulla is the converged engine for Medulla-owned data: property graph, SQL,
> RDF/OWL, vector, time series, provenance and reasoning over one durable authority
> and one query surface." — spec.md, "Outcome and boundaries"

**Consequence:** SQL federation, OBDA/R2RML virtual graphs, change capture and the
accelerated copy all project into this one engine's query surface (SPARQL, UQL,
Cypher, REASON) rather than standing up source-specific query paths.

## Decision 3 — Native hosting is earned per application through the `MEDULLA-UNIFIED-DATA-PLANE-R026` gate

Hosting an application's data natively in Medulla (rather than attaching its
existing database) is an explicit, per-application admission track, not a default
outcome of attaching a source.

> "Native hosting is an app-specific admission track, first measured with Gramps.
> Immich is an API connector and entity-resolution pilot owned by the connector
> repository; it is not a native-hosting candidate." — spec.md, "Outcome and
> boundaries"
>
> "This spec does not authorize a production cutover... P6 Gramps cutover requires
> the operator's go-ahead after P0–P5 evidence." — spec.md, "Outcome and boundaries"

**Consequence:** the Gramps pilot (`MEDULLA-UNIFIED-DATA-PLANE-R026`, phases P0–P5) is the
one evidence-gated path by which an application earns native hosting; no other
attached application is admitted natively without the same evidence and an explicit
operator go-ahead (`MEDULLA-UNIFIED-DATA-PLANE-R033`).

## Decision 4 — Medulla keeps no synced second store

Medulla keeps no synced second store for attached data. The attached source's own
database remains the write authority; Medulla's native store is the authority for
Medulla-owned data. A copy of attached data is allowed only as the watermark-bound
accelerated cache of `MEDULLA-UNIFIED-DATA-PLANE-R038` — a derived read path, never
a synced mirror.

> "Entity rows stay at the source (MEDULLA-UNIFIED-DATA-PLANE-R038). Metadata and
> hot subsets materialize; everything else reads live through approved mappings."
> — plan.md, Phase 1 design rule 2
>
> "The source database remains the write authority for attached application data.
> Medulla's native store is the authority for Medulla-owned data... A change-
> captured copy is a derived read path, selected only when its version and
> freshness meet the request's consistency contract." — spec.md, "Behavior detail"
>
> "No external Redis or graph mirror may be introduced as a read-path authority.
> No second connector registry, cache product or storage authority may be
> created." — spec.md, "Required outcomes and exclusions"

**Consequence:** mirror sinks (retired `MEDULLA-UNIFIED-DATA-PLANE-R002.3`) and the
shared relational platform are out of scope for this record; the
`MEDULLA-UNIFIED-DATA-PLANE-R038` watermark-bound accelerated copy is the only
materialization of attached data, and its freshness position is always declared in
the query plan.

## Decision 5 — The pgrx companion extension is a no-go

The pgrx companion extension is a no-go for this data plane: it is not a second
native storage authority growing in parallel with the attached-source data plane.

> "The pgrx companion remains a separate spike admitted only by ADR; this data-plane
> plan does not implement a second native store." — plan.md, "Contracts and data
> flow" §7
>
> "`durable-graph-kernel` owns the mixed graph/table atomicity ADR... The separate
> pgrx spike cannot become a parallel database authority by default." — plan.md,
> "Native transaction and storage dependency"

**Consequence:** this spec's attached-source and native-hosting work never depends on
pgrx; pgrx's own go/no-go is decided separately, by `MEDULLA-UNIFIED-DATA-PLANE-R025`'s
spike ADR.

## Consequences (summary)

These five decisions bound every requirement in `specs/unified-data-plane`: attach
first (1), through the one Medulla query surface (2), with native hosting earned per
application by evidence and operator go-ahead (3), with no synced second store —
the `MEDULLA-UNIFIED-DATA-PLANE-R038` watermark-bound accelerated copy is the only
materialization of attached data (4) — and never by implicitly growing the pgrx
spike into a second storage authority (5). A future change to any of these five
needs its own reviewed ADR, not a quiet reinterpretation of this one.
