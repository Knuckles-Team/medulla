# MEDULLA-BUSINESS-OPS — Business operations pack and deterministic rule evaluation

Owner: medulla. Cross-repo IDs to file: ARBORA-OPS (scheduling, approval queue serving, served digest operations), SCION-SDK (connector mappings into the pack), MERISTEM-HARNESS-CYCLE (role-agent runs that call these queries), OVERSTORY-APPS and TUIG (digest views).

## Outcome and boundary

A small organization, or one person, runs its operations through role agents on a small local model. The engine holds the organization's facts with full provenance. The engine also computes the answers that a small model computes badly: due dates, thresholds, matches, reorder points and phrase checks. The model asks a question. The engine returns a deterministic answer with the inputs it used.

Medulla owns the vocabulary, the stored rules, the evaluation and the queries. Medulla never schedules a run, never serves an approval queue and never executes an action. The caller supplies the as-of time. A rule never reads a clock.

The pack is business-agnostic. No class IRI, property IRI or rule name contains a brand, a product or an organization name. Every class rolls up to a core class.

## Why a new spec

Search: `git grep -n -iE "obligation|recurrence|reconcil|reorder|ApprovalMatrix|TaxReturn|Refund|Payout" -- specs crates/medulla-core/ontology`. Result: no spec owns an operations pack or rule evaluation over it. `commercial-v1.ttl` owns `:Customer`, `:PurchaseOrder` and `:PurchaseInvoice`. `supply_chain-v1.ttl` owns `:Supplier`. `finance-v1.ttl` owns accounts and ledger lines. `personal-v1.ttl` owns `:RecurringEvent`. Core owns `:Order`, `:Incident`, `:Policy` and `:LegalEntity`. This spec reuses all of them and adds only the missing terms. Action provenance extends `MEDULLA-DECISION-ENGINE-R869`. Tenant separation extends `MEDULLA-IDENTITY-R727`.

## Scope

Component: ontology and packs, UQL evaluation stages, stored queries.

Non-goals:
- The engine never moves money, files a return or publishes content.
- The engine never schedules evaluation. Arbora triggers it.
- The engine adds no `Method` variant for any question. Every question is a UQL query over the pack.

## User Scenarios & Testing

### User Story 1 - Hold the organization's facts in one pack (Priority: P1)

An operator loads orders, payments, batches and obligations from connectors. The engine stores them as typed nodes that roll up to core classes.

1. **Given** an order, a payment and a payout from three sources, **When** they are ingested, **Then** one query joins them with provenance per leg.
2. **Given** a class in the pack, **When** the census runs, **Then** it has a subclass path to a core class and no product name in its IRI.

### User Story 2 - Offload reasoning to the engine (Priority: P1)

A role agent asks "when is the next filing due" or "does this obligation apply at revenue X". The engine returns the answer and the inputs.

1. **Given** a monthly rule and an as-of time, **When** the agent queries, **Then** the next three due dates return with the holiday calendar revision.
2. **Given** copy text with one forbidden phrase, **When** the agent checks it, **Then** the verdict is `fail` with the matched span.

### User Story 3 - Answer the owner's weekly questions (Priority: P1)

The owner asks what needs approval, what is overdue and what changed this week. Each is a stored query over the pack.

1. **Given** two pending approvals and one overdue obligation, **When** the digest queries run, **Then** each returns its rows with the facts behind them.

### Edge Cases

- A rule with an unknown version refuses with `RULE_VERSION_UNKNOWN` and evaluates nothing.
- A phrase that is in neither set returns `needs_review`, never `pass`.
- A reconciliation with two equal candidates picks by the fixed tie-break and reports the tie.
- A request with no as-of time refuses with `ASOF_REQUIRED`.

## Requirements

- **FR-001**: One pack models profile, products, orders, money, procurement, compliance, tax, approvals, deploys and incidents. Rows: `MEDULLA-BUSINESS-OPS-R001`.
- **FR-002**: Five rule kinds evaluate deterministically through UQL. Rows: `MEDULLA-BUSINESS-OPS-R002`.
- **FR-003**: The owner digest answers as stored queries with as-of reads. Rows: `MEDULLA-BUSINESS-OPS-R003`.
- **FR-004**: Action provenance is queryable. Rows: `MEDULLA-DECISION-ENGINE-R869`.
- **FR-005**: Business and personal tenants stay separate with explicit sharing. Rows: `MEDULLA-IDENTITY-R727`.

## Success Criteria

- **SC-001**: Every rule kind returns the same bytes for the same inputs on two architectures.
- **SC-002**: No rule evaluation reads a wall clock.
- **SC-003**: No pack class lacks a core subclass path.

## Cross-repository rows to file

| Owner | Needed row |
|---|---|
| ARBORA | Trigger recurrence evaluation on a schedule and call the digest queries. |
| ARBORA | Serve the approval queue and record the approver's signed decision. |
| SCION-SDK | Provide the generic order, payment, payout and stock mapping helpers. |
| CONN-* | Map each external system to the pack classes. |
| MERISTEM | Let a role run call the stored queries and cite the returned inputs. |
| OVERSTORY, TUIG | Render the digest and the "why" chain. |
