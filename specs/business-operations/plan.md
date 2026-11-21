# MEDULLA-BUSINESS-OPS — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

One ontology file, `crates/medulla-core/ontology/business_ops-v1.ttl`, and its shapes `business_ops-v1.shapes.ttl` add the missing terms. The file registers in `GraphSchemaSources` beside `commercial-v1.ttl` and `supply_chain-v1.ttl`. Five rule kinds live in one new module of `crates/medulla-numeric` and register in the `NumericEvaluate` registry. The UQL `EVALUATE` stage calls the registry. No new `Method` exists.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain; Turtle ontology files.
- **Primary Dependencies**: `crates/medulla-core`, `crates/medulla-numeric`, `crates/medulla-h-query`, `crates/medulla-exec`, `crates/medulla-h-ingest`. No new third-party dependency.
- **Storage**: the engine graph store through the existing `SourceIngest` commit; no second store.
- **Testing**: ontology tests in `crates/medulla-core/tests/business_ops_vocab.rs` (L2), kernel tests in `crates/medulla-numeric` (L1) and served tests in `crates/medulla-server-served-tests` (L3).
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Ontology files and a numeric module in existing crates.
- **Performance Goals**: a rule evaluation over 10,000 input nodes completes in 50 ms.
- **Constraints**: no clock, no randomness, no model call and no new `Method` variant.
- **Scale/Scope**: 25 rows over 3 user stories, 5 `FR-` labels and 3 `SC-` labels.

## Constitution Check

- **The engine is native Rust and the only fact store**: pass. Rules and queries run in the engine.
- **Every wait has a deadline**: pass. Evaluation is pure and bounded by input size.
- **Every row sets budgets and names a benchmark**: pass. The 50 ms budget applies to each kind.
- **Crates are small, layered and cut over hard**: pass. The new module sits in `medulla-numeric`; no handler calls a sibling handler.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

- `crates/medulla-core/ontology/business_ops-v1.ttl` and `business_ops-v1.shapes.ttl` (new).
- `crates/medulla-numeric/src/business/` with `rule_spec.rs`, `recurrence.rs`, `threshold.rs`, `reconcile.rs`, `reorder.rs`, `phrase_policy.rs` (new).
- `crates/medulla-numeric/src/evaluation/registry.rs` (five registrations).
- Tests as named in `test-spec.md`.

## Phase 0: Research

No unknown remains. The existing packs, the numeric registry and the UQL stages cover every need. The reuse inventory below records the search.

## Phase 1: Design

The interfaces below fix the rule record, the result shape and the money rule. The data shapes follow the row text in `requirements.md`.

## Phase 2: Task approach

`tasks.md` holds one task per row, producer first. Vocabulary precedes rules, rules precede wiring, wiring precedes the digest.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- The module reuses the numeric registry; the no-duplicate-authority check finds no parallel one.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- `Method::Uql` to `dispatch_query_family` (`crates/medulla-h-query/src/query/dispatch.rs`) to the UQL `EVALUATE` stage to `registry::evaluate` to `crates/medulla-numeric/src/business/`.
- `Method::SourceIngest` to `crates/medulla-h-ingest/src/source_ingestion.rs` for pack facts.
- `GraphSchemaSources::default()` to `current_core_catalog` to the `business_ops` module for the vocabulary.
- Obsolete paths to delete: none; every path above is new.

## Cross-repo seams

- Scheduling and digest calls: `MEDULLA-BUSINESS-OPS-R003` provides stored queries; an ARBORA row (to file) consumes them.
- Connector mappings: the pack classes of `MEDULLA-BUSINESS-OPS-R001` are the target of connector rows (to file).
- Action provenance and tenant sharing: `MEDULLA-DECISION-ENGINE-R869` and `MEDULLA-IDENTITY-R727` extend this pack's scope.

## Reuse inventory

| Need | Existing owner | Action |
|---|---|---|
| Customer, purchase order, purchase invoice | `commercial-v1.ttl` | Extend with lines and receipts |
| Supplier, supply item | `supply_chain-v1.ttl` | Reuse |
| Order, Incident, Policy, LegalEntity, Role, Decision, Evidence | `core-foundation-v1.ttl` | Subclass |
| Recurring event, calendar event | `personal-v1.ttl`, `calendar-v1.ttl` | Reuse for display only |
| Ledger line, budget, account | `finance-v1.ttl` | Reuse; money rule of `MEDULLA-FINANCE-PRIMITIVES` |
| Invocation record | `action-v1.ttl` | Extend in `MEDULLA-DECISION-ENGINE-R869` |
| Numeric kernel registry and UQL stages | `MEDULLA-FINANCE-PRIMITIVES-R002.2.5`, `MEDULLA-FEDERATED-QUERY` | Register five kinds |
| Valid and known time | `temporal-v1.ttl`, `:ValidityInterval` | Reuse |

## Interfaces

- Rule records are graph nodes `:RuleSpec { kind, version, canonical digest, parameters }`. A rule is immutable by id and version.
- Every evaluation takes an explicit as-of time and returns `{ result, inputs[], rule_digest, trace }`. The inputs are node ids and revisions.
- Money is an integer in minor units with an ISO currency code. No binary float enters a persisted amount.

## Live wiring

Entry: `Method::Uql` → `dispatch_query_family` → UQL `EVALUATE` stage → `crates/medulla-numeric/src/evaluation/registry.rs::evaluate` → `crates/medulla-numeric/src/business/` (new, one file per kind). The natural-language path compiles to the same UQL. Medulla adds no second route.

## Decisions

- One rule module, five kinds, one registry. A sixth kind is a new row here.
- A stored query is a graph node that the caller names. The engine stores no per-question API.
- The pack carries no jurisdiction data. A `HolidayCalendar` and a `TaxRegime` are facts the operator ingests.
