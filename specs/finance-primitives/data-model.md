# MEDULLA-FINANCE-PRIMITIVES — Data model

This file describes the entities the finance rows add or extend. Classes and shapes live once in `crates/medulla-core/ontology/finance-v1.ttl` and `finance-v1.shapes.ttl`; wire types live in `crates/medulla-types`.

## Entities

### Account

- Fields: tenant, owner, base currency, account kind, source identity, policy scope, schema version.
- Owner: `crates/medulla-types/src/wire_finance.rs` (kind `FinanceRecordKind::Account`) and the `AccountShape` of the shapes file.
- Rule: client-writable; every key includes the tenant.

### Activity

- Fields: stable source ID, account, instrument or listing, type (`buy`, `sell`, `dividend`, `split`, `fee`, `transfer`, `interest`, `deposit`, `withdrawal`), quantity, price, currency, effective time, `known_at`, source revision and digest.
- Relation: belongs to one `Account`; the append-only journal is the authority for holdings.

### Lot and Position

- Fields: acquisition time, remaining quantity, native-currency cost and source activity (`Lot`); quantity, valuation source, as-of time and currency (`Position`).
- Rule: both are derived from activities and are never independently writable; a write returns `RECORD_DERIVED_READONLY`.

### PortfolioGroup, StrategySpec, StrategyRun, LeverageTerms

- Fields: schema version, tenant, visibility, source, effective time and known-at time, plus the kind-specific fields named in `MEDULLA-FINANCE-PRIMITIVES-R004.2.3` to `MEDULLA-FINANCE-PRIMITIVES-R004.2.6`.
- Rule: `StrategySpec` is immutable by `(id, version, canonical digest)`.

### FinanceReceipt

- Fields: `idempotencyKey`, tenant, subject, outcome digest.
- Rule: written once through `Method::CreateNodeIfAbsent`; an empty key returns `RECEIPT_KEY_INVALID`.

### Factor

- Fields: factor ID, information coefficient, information ratio, rank, `irKernelVersion`, `icKernelVersion`.
- Relation: `derivedFrom` edges to each input in `provenance_keys`; an empty provenance list is refused.

### BacktestRun and AnalysisSnapshot

- Fields: draft, validation outputs, canonical digest, provenance, share-lease link (snapshot).
- Owner: `crates/medulla-finance-market/src/market/`; both are sealed classes that a generic write cannot overwrite.

### OrderProposalSnapshot

- Fields: tenant, proposal, revision, predecessor digest, decision identity, revision and digest, schema identity.
- Relation: revisions chain by predecessor digest; promotion writes one receipt and history entry in one transaction.

### FinanceAlertEnvelope

- Fields: `event_id`, `kind`, `listing_id`, `occurred_at_unix_nanos`, `message`.
- Owner: `crates/medulla-types/src/finance_event_envelope.rs`; `finance.flip` keeps its stable event ID.

### StateSpaceModel and StateSpaceCheckpoint

- Fields: model revision, kernel revision, noise policy, symmetry tolerance, numeric policy; checkpoint holds mean, covariance and cursor.
- Owner: `crates/medulla-types/src/series_expr.rs`; the checkpoint persists in the existing derived state.
