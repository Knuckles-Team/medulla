# source-authority — Research

## Decision: Authority comes from the typed contract

**Decision.** `SourceAuthority` is detected from the typed `RecordContract` carried by `AttachApproved`. The Turtle path is removed (hard cutover). `medulla-h-admin` gains the `medulla-virtual-graph` dependency, and the governed-ledger harness (`MEDULLA-SRC-AUTH-R003.6`) is built for the served test.

## Decision: where served SourceCoverage lives

**Decision.** The wire reply and its verdict type live in `medulla-identity-types` beside `AuthorityAudit`, which the reply embeds (`medulla-types` cannot see that crate). The handler lives in `medulla-h-query`, whose context holds the request graph's core, the schema sources and the verified carrier; the foreign-source dispatch in `medulla-h-sparql` has none of these. The verdict leaves as its own serializable struct with premises rendered to text; `CoverageResult` and `Premise` stay internal to selection. A source is connected only when it is in the caller's own owner-scoped registry and an approved mapping of the schema sources names it; the request graph is the selection partition and the verified tenant the selection scope. The two limits are the registry knobs `source_coverage.hop_budget` and `source_coverage.max_classes`.
