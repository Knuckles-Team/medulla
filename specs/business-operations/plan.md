# MEDULLA-BUSINESS-OPS — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

One ontology file, `crates/medulla-core/ontology/business_ops-v1.ttl`, and its shapes `business_ops-v1.shapes.ttl` add the missing terms. The file registers in `GraphSchemaSources` beside `commercial-v1.ttl` and `supply_chain-v1.ttl`. Five rule kinds live in one new module of `crates/medulla-numeric` and register in the `NumericEvaluate` registry. The UQL `EVALUATE RULE` stage calls the registry. No new `Method` exists.

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
- `crates/medulla-numeric/src/business/registry.rs` (`RuleInputs`, `RuleResult`, `evaluate_rule`: one match arm per kind).
- `crates/medulla-query-types/src/rule_expr.rs`, `crates/medulla-uql/src/parser/evaluate.rs` and `crates/medulla-exec/src/exec/evaluate_rule.rs` (the `EVALUATE RULE` stage).
- `crates/medulla-source-types/src/business_digest.rs` (the three digest stored queries).
- Tests as named in `test-spec.md`.

## Phase 0: Research

One unknown remained after the reuse search; [research.md](research.md) resolves it.

1. Which existing authority hosts deterministic business rules so that the query path reaches them with no new method?

## Phase 1: Design

Design answers:

- **Owner.** Medulla. The vocabulary lives in `medulla-core`, the rule kinds in `medulla-numeric` and the stages in `medulla-h-query` and `medulla-exec`.
- **Cohesion.** One pack, one rule record, one result shape and one registry, so every kind shares the as-of rule and the input list.
- **Dependency direction.** The UQL stage points down to the registry in `medulla-numeric`; the numeric crate reads no handler and no store.
- **Interface depth.** One `EVALUATE RULE` stage over one rule registry; a new rule kind is a new `RuleKind` variant and match arm, not a new method.
- **Special cases.** None per caller. Missing data returns `unknown` or `insufficient_data` in every kind, never a guess.
- **State.** The rule kinds hold no state between calls; the graph holds `:RuleSpec`, instances and approvals.
- **Environment.** The rules read no environment variable and no clock; the caller supplies the as-of time.

### Reuse inventory

| Need | Existing owner | Action |
|---|---|---|
| Customer, purchase order, purchase invoice | `commercial-v1.ttl` | Extend with lines and receipts |
| Supplier, supply item | `supply_chain-v1.ttl` | Reuse |
| Order, Incident, Policy, LegalEntity, Role, Decision, Evidence | `core-foundation-v1.ttl` | Subclass |
| Recurring event, calendar event | `personal-v1.ttl`, `calendar-v1.ttl` | Reuse for display only |
| Ledger line, budget, account | `finance-v1.ttl` | Reuse; money rule of `MEDULLA-FINANCE-PRIMITIVES` |
| Invocation record | `action-v1.ttl` | Extend in `MEDULLA-DECISION-ENGINE-R869` |
| UQL stage table, parser and executor dispatch | `MEDULLA-FEDERATED-QUERY` | Add the `EVALUATE RULE` form beside the kernel form; the array kernels of `NumericEvaluateOp` stay untouched |
| Stored queries | `MEDULLA-FEDERATED-QUERY-R708` (`StoredQueryPut`, `StoredQueryCall`) | Define the three digest queries |
| Valid and known time | `temporal-v1.ttl`, `:ValidityInterval` | Reuse |

### Interfaces

- Rule records are graph nodes `:RuleSpec { kind, version, canonical digest, parameters }`. A rule is immutable by id and version.
- Every evaluation takes an explicit as-of time and returns `{ result, inputs[], rule_digest, trace }`. The inputs are node ids and revisions.
- Money is an integer in minor units with an ISO currency code. No binary float enters a persisted amount.

### Live wiring

Entry: `Method::Uql` → `dispatch_query_family` → `handle_uql` → `medulla_exec::exec::apply` → `evaluate_rule::apply` (`crates/medulla-exec/src/exec/evaluate_rule.rs`) → `crates/medulla-numeric/src/business/registry.rs::evaluate_rule` → one file per kind in `crates/medulla-numeric/src/business/`. The natural-language path compiles to the same UQL. Medulla adds no second route.

### Decisions

- One rule module, five kinds, one registry. A sixth kind is a new row here.
- A stored query is a graph node that the caller names. The engine stores no per-question API.
- The pack carries no jurisdiction data. A `HolidayCalendar` and a `TaxRegime` are facts the operator ingests.
- The rule kinds do not enter `NumericEvaluateOp`: those kernels take arrays a request carries, while a rule reads graph nodes and must name their revisions. One stage form, `EVALUATE RULE`, is the only route to a rule kind.
- A rule's configuration lives only in `:ruleParameters` of its `:RuleSpec` node, so the rule digest covers exactly what is evaluated. `:RecurrenceRule` and `:ReorderPolicy` are `:RuleSpec` subclasses; their former field properties are deleted.
- Every sentence of a claim text is a claim (`R002.6`); there is no claim-marker list.
- The digest queries are `StoredQuery` records defined once in `business_digest.rs`; the consumer puts them and calls them by name.

### Gather table of the `EVALUATE RULE` stage (R002.7)

Every input row is one node of `ctx.view`. Node properties are read by their pack local names. `tx_from` of a node is its revision (0 when absent). A node's day is `valid_from` (epoch seconds) `div_euclid(86_400)`; the as-of day is `as_of_ms.div_euclid(86_400_000)`. A row of a class the kind does not take is ignored; a missing required property refuses `RULE_INPUT_INVALID` naming the node and the property.

| Kind | Parameters (`:ruleParameters` names) | Input rows | `RuleInputs` arm |
|---|---|---|---|
| `recurrence` | `frequency` (`daily`, `weekly`, `monthly`, `quarterly`, `yearly`), `interval` (integer at least 1), `day` (`1` to `31`, `last`, or `<n>:<weekday>` with `n` 1 to 5 and weekday `mon` to `sun`), `months` (optional, comma list of 1 to 12), `adjustment` (`none`, `next_business_day`, `previous_business_day`), `zone` (IANA name), `anchor` (`YYYY-MM-DD`) | at most one `:HolidayCalendar` row, its `holidayDate` values (`YYYY-MM-DD`, a list) as the holidays and its `tx_from` as the revision; none means no holidays | `Recurrence { calendar: HolidayCalendar, count: usize }`, `count` = `COUNT` (default 1; `next_due` refuses outside 1 to 60) |
| `threshold` | `expr` (grammar below), `metrics` (comma list of `name:series:agg`, `agg` one of `sum`, `count`, `max`), `period_days` (integer at least 1) | `:LedgerEntry`, `:Order`, `:Payment` rows; each is one `DataPoint { series: <class local name>, day, amount }`; `amount` is `amountMinor`, or for an `:Order` the order amount below | `Threshold { points: Vec<DataPoint> }` |
| `reconcile` | `tolerance_minor` (integer), `window_days` (integer), `currency` (`same` or a three-letter code) | `:Order` (amount = sum over its `:OrderLine` nodes linked by `:lineOfOrder` of `lineQuantity * unitPriceMinor + taxMinor`, currency `amountCurrency` of the order), `:Payment`, `:Payout` (`amountMinor`, `amountCurrency`), `:Refund` (`:refundOf` target as `payment_id`), `:Fee` (`:feeOfPayout` target as `payout_id`) | `Reconcile { sets: Sets }` |
| `reorder` | `lead_time_days`, `safety_stock`, `minimum_order_quantity`, `review_window_days`, `minimum_samples` (integers) | `:OrderLine` rows as `Sale { day, quantity: lineQuantity }`, exactly one `:StockLevel` (`onHand`, `reservedQuantity`), `:PurchaseOrderLine` rows as `OpenLine { quantity: purchaseLineQuantity }` | `Reorder { sales, stock, open_lines }` |
| `phrase_policy` | none | exactly one `:ClaimPolicy` row; its phrases are the targets of its `:claimForbiddenPhrase` and `:claimApprovedPhrase` edges (`phraseText`, `phraseLocale`, `phraseMatchMode`); its `tx_from` is the policy revision | `PhrasePolicy { text: TEXT, locale: LOCALE or empty, policy }`; no `TEXT` refuses `RULE_INPUT_INVALID` |

Threshold `expr` grammar: `expr := conj { " or " conj }`, `conj := atom { " and " atom }`, `atom := metric " " op " " int | metric " between " int " " int`, `op := "<" | "<=" | ">" | ">="`; it maps to `Expr::Or`, `Expr::And`, `Expr::Compare` and `Expr::Between`, folded left. The `holidayDate` property (`owl:DatatypeProperty`, domain `:HolidayCalendar`, range `xsd:date`) is added to `business_ops-v1.ttl` by this row.

`RuleResult::canonical_json` keys per kind: `recurrence` `{zone, calendar_revision, dates: [{scheduled, due}]}`; `threshold` `{applies: "yes"|"no"|"unknown", metrics: [{name, value (integer or null), nodes}]}`; `reconcile` `{matches: [{kind: "order_payment"|"payment_payout", left, right, residual_minor, date_gap}], unmatched: [{kind, id, reason}], ties: [{kind, anchor, chosen, other}]}`; `reorder` `{status, reorder, point (integer or null), position, quantity, samples}`; `phrase_policy` `{verdict, forbidden: [{sentence, start, end, phrase}], unmatched, policy_revision}`. Enum values are snake_case.

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

- `Method::Uql` to `dispatch_query_family` (`crates/medulla-h-query/src/query/dispatch.rs`) to the UQL `EVALUATE RULE` stage to `registry::evaluate_rule` to `crates/medulla-numeric/src/business/`.
- `Method::SourceIngest` to `crates/medulla-h-ingest/src/source_ingestion.rs` for pack facts.
- `Method::StoredQueryPut` and `Method::StoredQueryCall` to `crates/medulla-h-query/src/query/stored_query.rs` for the digest queries.
- `GraphSchemaSources::default()` to `current_core_catalog` to the `business_ops` module for the vocabulary.
- Obsolete paths to delete: the field properties of `:RecurrenceRule` and `:ReorderPolicy` named in `R002.7`.

## Cross-repo seams

- Stored queries and rule results: `MEDULLA-BUSINESS-OPS-R003` provides, an ARBORA row (to file) consumes. Contract: [arbora-digest-queries.md](contracts/arbora-digest-queries.md).
