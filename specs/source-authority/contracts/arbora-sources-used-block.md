# Seam: sources used and why

Provider: medulla MEDULLA-SRC-AUTH-R007.4
Consumer: arbora ARBORA-FLEET-R052.3

## Wire shape

Medulla returns `SourceCoverageReply { verdicts, audit }` from `Method::SourceCoverage`, and the same entries as `sources_used` in the cross-source report. Each audit entry names the source, the concept, an optional facet, the role, the premises as text, the mode (`live` or `accelerated`) and, for a row not used, its reason. Arbora prints the block from `audit` and adds nothing to it.

## Refusal codes

`SOURCE_COVERAGE_NO_CLASSES`, `SOURCE_COVERAGE_TOO_MANY_CLASSES` and `SOURCE_COVERAGE_HOP_BUDGET_INVALID`. An abstained verdict is a result, not a refusal; Arbora prints its reason.

## Authoritative types

`AuthorityAudit`, `AuditEntry`, `AuditSourceMode`, `NotUsedReason` and `SourceRole` in `crates/medulla-identity-types/src/authority_audit.rs`.

## Test vectors

The served tests named by `MEDULLA-SRC-AUTH-R009.4` and `MEDULLA-SRC-AUTH-R007.4` are the vectors; Arbora proves its rendering against the same reply type.
