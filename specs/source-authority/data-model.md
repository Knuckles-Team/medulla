# MEDULLA-SRC-AUTH — Data model

This file describes the entities that source authority adds: the ontology row, the verdict and its premises, the audit, the trust mark and the policy binding.

## Entities

### vg:SourceAuthority

- Fields: `authoritativeSource`, `authorityFor`, `authorityMapping`, `authorityRank`, `authorityContract`, optional `authorityFacet`, `scopeTenant`, `scopePartition`, `freshnessMaxAgeSeconds`, `allowFallback`, `materialisation`, `materialisationReason`, `approved`.
- Owner: `crates/medulla-core/ontology/virtual_graph-v1.ttl`; shapes in `virtual_graph-v1.shapes.ttl`.
- Rule: only an operator-origin or approved row is effective.

### AuthorityRow and CoverageResult

- Fields: the row parsed into class, mapping, source, generation, rank, scope, origin, approval and fallback flags; a verdict is `Covered`, `Abstained`, `Uncovered` or `Disconnected`.
- Owner: `crates/medulla-virtual-graph/src/source_selection.rs`.
- Relation: each verdict carries a `Premise` list; the audit is built from it only.

### AuthorityAudit

- Fields: used entries and not-used entries, each with source, concept, facet, role, premises, mode and reason.
- Owner: `crates/medulla-identity-types/src/authority_audit.rs`.
- Rule: the wire refuses unknown fields and open values.

### TrustMark

- Fields: `trusted` or `untrusted`, `markedBy`, `markedAtGeneration`, optional approval id.
- Owner: `crates/medulla-virtual-graph/src/source_selection/trust.rs`.
- Rule: lowering requires an operator approval identifier.

### SourceCoverageReply and PolicySourceBinding

- Fields: verdicts and audit; source reference, revision, digest, policy IRI, approver and bind time.
- Owners: `crates/medulla-types/src/source_coverage.rs` and `crates/medulla-types/src/policy_source.rs` (new).
