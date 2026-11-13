# Research: connector competency

## Served vocabulary coverage reads source records through Uql, not SPARQL (MEDULLA-CQ-CONN-R089)

Question: how does the served vocabulary test read back a record it ingested?

Finding: `SourceIngest` writes each record as a property-graph node whose id is
`source:<digest>`. The SPARQL projection (`export_view_triples`) emits only nodes whose id is an
RDF term (`<iri>` or a blank node), so an ingested source record does not appear in served SPARQL.

Decision: the test reads the record through served `Uql` (`MATCH () WHERE type = '<pack class>'`)
and the node properties (`core#sourceRecordId`, `source_stream`, `core#sourceUpdatedAt`,
`source_provenance`, and the value under the term IRI). The term's declaration is proved by the
served `ConnectorPack.Import`: a pack class that extends a core class the catalog lacks is refused
`ONTOLOGY_ORPHAN`. Projecting source records into the RDF view is a separate engine feature and
is not needed for the vocabulary proof.

## Virtual-graph provenance (MEDULLA-CQ-CONN-R067.5)

Question: where does a `SparqlVirtual` row get its mapping IRI and fetch time?

Decision: the mapping IRI is declared by the mapping itself as an `rr:constant` object map, so the
engine returns it on every row; the host stamps the fetch time around the call, as the other
contract-kind journeys do.

## MEDULLA-CQ-CONN-R090: one served statement over several sources

Question: which served surface carries a federated read over two virtual graphs and engine facts, with
a row-level source citation and a coverage block?

Decision: `Method::Uql`, the one query-text surface. `UnifiedQuery` returns a bare `[id, score]` list
with no place for provenance or coverage; `Uql` already carries each source row's provenance map
(R722.5). `Method::Uql` takes `foreign_executions` (a bounded list, as `UnifiedQuery` does) in place of
the single `foreign_execution` (DECISIONS FQ-UQL-MULTI, hard cutover; the Python `uql` takes
`foreign_executions`). The row's entry chain names `Uql` instead of `UnifiedQuery`; the plan legs and
the foreign binding are the same.

Question: what happens to a source the caller cannot read?

Decision: when a statement carries foreign executions, every source it names that the caller's
registry cannot resolve is bound as an empty source (so the plan keeps its shape and adds no rows) and
is listed in `UqlResult::Rows.source_coverage` as `used: false` with the reason `unconnected`, the
source-authority `NotUsedReason` for a source absent from the caller's owner-scoped catalog. A source
another owner holds and a source that does not exist are the same case, so the entry never confirms
that a source exists (DECISIONS NO-EXISTENCE-LEAK). Readable sources answer and are listed as used.
A statement with no executions keeps today's typed not-found.

Question: a write stage?

Decision: UQL has no write stage in its grammar, so a statement with one fails to parse before the
text digest check and before any execution is bound; no source is called.
Question: what makes a foreign source keep a cache (MEDULLA-CQ-CONN-R067.6)?

Decision: the fragment cache was keyed only on a fresh connector checkpoint, so any checkpointed source
was cached whether or not anyone declared it. It now serves a named source only when an effective
`vg:SourceAuthority` row with `vg:materialisation "accelerated"`, a reason and a freshness bound sits on a
connection that declares the `copy` capability (`CoverageFixture::declared_caches`; a pack or ingestion
row is not effective, SRC-AUTH-R003.4). The declared `vg:freshnessMaxAgeSeconds` caps how long an entry
serves. An undeclared source is virtual and disables fragment caching for that query. The served test
observes the cache by changing a row at the source between two reads, so no new read of the cache exists.
# Research: connector competency (current decisions)

## R090: one federated read over two virtual graphs and engine facts

- **Result shape.** The served `UnifiedQuery` answers `UnifiedQueryAnswer`: rows (id, score, and the
  named provenance channels a source attached) plus a coverage block (`used`, `not_used`). A row
  of the engine's own graph carries no provenance. This is the same projection `Method::Uql` uses,
  so there is one way a row cites its source.
- **Union of sources.** A fused plan (`FuseRrf`) keeps the text channels of every branch that
  returned a row. Execution pins are matched against every source the plan names, including
  sources inside a fused branch.
- **Per-source access.** The caller's owner-scoped registry is the one place that decides what is
  readable. A named source it does not hold is left out and reported in `coverage.not_used` with
  reason `unconnected`: another owner's source and a missing source look the same (no existence
  leak). Its reads become empty reads, so it is never called and adds no rows; a joined read
  narrows to nothing. The query is not refused.
- **Write stage.** The plan algebra has no write operator. A source statement is the only place a
  write can enter, so an inline SQL source whose statement is not exactly one read refuses with
  `UNIFIED_QUERY_WRITE_STAGE`, before any leg binds and before any source is called.
- **Plan pin.** The query digest of a unified execution is the plan digest `ExplainPlan` reports.
## R084.6: question 1 through Decide, in a fresh graph

Question: how does the closing served proof run question 1 through `Decide` kind `capability_designation` without disturbing the shared graph?

Decision: the same case imports the finance pack a second time under a derived connector name into a fresh graph, attaches it there, and asks the served designation for the target class `:Position` (the class the pack's risk tools operate on). It reuses the commons generation and schema helpers of the R172.12.6 served test, so there is one way to establish the commons embedding space. The answer must name exactly the two tools that operate on `:Position`, each with a `target_designation` premise that cites the pack, and never the tool that operates on `:FinancialInstrument`. The missing-pack abstention for question 1 is asserted in the same test before the pack is imported (`PACK_NOT_IMPORTED`, naming the connector), using the served `requires_packs` check of R084.7.
