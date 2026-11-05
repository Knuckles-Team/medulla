# MEDULLA-TYPED-PACKS — Data model

This file describes the entities that the rows of `requirements.md` add or rely on: the pack wire values, the prompt identity, the schema source, and the catalog and object-model records.

## Entities

### ConnectorPackIndex and PackEntry

- Fields: connector, schema version, one server entry, sorted unique typed entries and bounded archive sections; each entry has a kind, URI, name, media type, section, annotations and sibling references.
- Owner: `crates/medulla-catalog-types/src/connector_pack/index.rs` (`ConnectorPackIndex`, `PackEntry`, `PackEntryKind`).
- Rule: the entry kind set is closed; every section carries offset, length and SHA-256; Medulla recomputes every digest and refuses a mismatch.

### Pack head and member records

- Fields: head keyed by tenant and connector with binding revision, content digest, server pin, package provenance, record id and projection state; members keyed by pack URI with component id, revision, content digest and lifecycle.
- Owner: `crates/medulla-catalog-types/src/connector_pack/record.rs` and the rows of `crates/medulla-persistence-records/src/connector_pack/`.
- Relation: members belong to one head; body holders protect engine bodies keyed by SHA-256.

### PromptBlueprintIdentity

- Fields: tenant, component id, entry revision, schema revision, body SHA-256 and provenance.
- Owner: `crates/medulla-agent-types/src/agent_component.rs`.
- Rule: a changed schema or body pin changes the identity; an unsupported version or missing pin refuses with `PROMPT_IDENTITY_INVALID`.

### GraphSchemaSource

- Fields: source id, origin, revision, shapes and ontology bytes; the composed digest and policy are derived.
- Owner: `crates/medulla-graph-core/src/graph/schema_sources.rs`, with the served views in `crates/medulla-catalog-types/src/graph_schema.rs`.
- Rule: each shape subject has one owning source; a pack or ingestion revision strictly increases or is an identical no-op.

### PackContractVersion and ContractBreak

- Fields: connector, version, contract digest, ontology digest, shapes digest and the superseded version; a break has a kind and an item.
- Owner: `crates/medulla-catalog-types/src/connector_pack/contract_version.rs`.
- Relation: a re-import compares releases with the ontology diff and the schema-break rule; a breaking release stays pending behind a change request.

### ConformanceEvidence

- Fields: pack digest, tool fingerprints, issued, expiry and revocation times, per tenant, connector and immutable revision.
- Owner: `crates/medulla-catalog-types/src/connector_pack/conformance_evidence.rs` (new).
- Rule: the write path is the connector onboarding import; a read of a missing or revoked record refuses `CONFORMANCE_EVIDENCE_ABSENT`.

### ValueType, LinkType and OntologyEdit

- Fields: a value type has base, pattern, bounds and allowed values; a link type has endpoint classes and cardinalities; an edit has kind, subject, before, after and previous digests.
- Owner: the object-model and ontology-edit modules named by the rows `MEDULLA-TYPED-PACKS-R650.1` and `MEDULLA-TYPED-PACKS-R651.1` (new).
- Rule: value and link types compile to enforced shapes; the edit chain is append-only and each inversion is exact.

### ClassificationClaim

- Fields: asset, categories, provenance, state and evidence.
- Owner: `crates/medulla-types/src/classification.rs` (new, `MEDULLA-TYPED-PACKS-R652.1`).
- Rule: a claim moves only from proposed to promoted or rejected; a promoted marking propagates along lineage edges as proposed claims.
