# MEDULLA-BUSINESS-OPS — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

One ontology file, `crates/medulla-core/ontology/business_ops-v1.ttl`, and its shapes `business_ops-v1.shapes.ttl` add the missing terms. The file registers in `GraphSchemaSources` beside `commercial-v1.ttl` and `supply_chain-v1.ttl`. Five rule kinds live in one new module of `crates/medulla-numeric` and register in the `NumericEvaluate` registry. The UQL `EVALUATE` stage calls the registry. No new `Method` exists.

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
