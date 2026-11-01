# MEDULLA-INCREMENTAL-VIEWS — Research

Governing spec: [spec.md](spec.md). Plan: [plan.md](plan.md).

Each entry states a decision, its rationale and the alternatives considered. All findings are in this platform's terms.

## Techniques

- **Incremental view maintenance.** A view keeps its result and applies only the change of its inputs. The cost of one step follows the size of the change, not the size of the view.
- **Weighted multiset deltas.** A change is a multiset of rows with signed integer weights. Weights of equal rows add. A zero weight removes the row. Linear operators such as filter and projection map a delta to a delta directly.
- **Change-propagation circuits.** A view plan compiles into a chain of stateful operators. Each operator turns an input delta into an output delta and updates its own state.
- **Join by the product rule.** The change of a join of `A` and `B` is `dA ⋈ B' + A' ⋈ dB - dA ⋈ dB` over the post-change inputs `A'` and `B'`.
- **Fixed-point iteration over deltas.** A recursive closure iterates on the new facts of the last round only. A retraction over-deletes the dependent facts and re-derives the facts that keep support.
- **Differential oracle testing.** A test applies random changes and compares the maintained result with a full recompute after every step.
- **State budgets and spill.** Each operator state has a measured size. A budget bounds resident state and moves cold state to the store.

## Decisions

### D1. Owner: one new spec for maintenance

- Decision: file the rows in `specs/incremental-views/`, ID `MEDULLA-INCREMENTAL-VIEWS`. Cite MEDULLA-ENGINE-PARITY-R014, R015 and MEDULLA-FEDERATED-QUERY-R708 by ID.
- Rationale: no row owns the shipped kernel. The kernel serves plan views, SQL views and the closure, so one owner keeps one place for its rules.
- Alternative considered: children under MEDULLA-FEDERATED-QUERY-R708 and MEDULLA-ENGINE-PARITY-R014 and R015. Rejected for two reasons. R708 is the stored-query rollup, and its root test does not cover maintenance. Both specs predate the current standard. Extending them requires converting every task line, and that marks over 500 legacy rows as changed in the same change.

### D2. SQL row-change events are not implemented

- Finding: no `InsertRow`, `UpdateRow`, `DeleteRow` or `emit_row_changes` exists. `CdcKind` has only node and edge variants (`crates/medulla-types/src/wire_streaming.rs:17`).
- Finding: the cited paths of MEDULLA-ENGINE-PARITY-R015.1 and R015.2 moved. `commit_txn_batch_result` is in `crates/medulla-table-store/src/tables/store.rs:2019`. The SQL commit caller is `crates/medulla-h-query/src/query/sql_catalog.rs:267`. `CdcHub::emit` is in `crates/medulla-server-cdc/src/cdc.rs:269`.
- Decision: R009, R010.1 and R010.2 depend on R015.1 and R015.2. They are not in the first slice.

### D3. `CREATE MATERIALIZED VIEW` is not stored as a materialized view

- Finding: `classify_create_view` refuses a plain `CREATE MATERIALIZED VIEW` (`crates/medulla-sql-lang/src/sql/classify.rs:2216`). Only the continuous-aggregate form is accepted. It stores a plain view through `TxnOp::CreateView` (`crates/medulla-h-query/src/query/sql_dispatch.rs:512`).
- Finding: `TableStore::create_matview` and the file `matview.rs` named by MEDULLA-ENGINE-PARITY-R014.1 do not exist. The table store now lives in `crates/medulla-table-store/src/tables/`.
- Decision: R009 depends on MEDULLA-ENGINE-PARITY-R014.1 and R014.2 for the catalog, the result table and the refresh path.

### D4. SQL maintenance runs inside the writing transaction

- Decision: `maintain_views` runs in `apply_mutation_txn_ops_with_crashpoints`, in the same redb write transaction.
- Rationale: the view commits with its inputs, so no read sees a base change without its view change. No stale flag and no wait exist.
- Alternative considered: maintain after the commit from the change feed. Rejected: a reader could see the new base rows and the old view.

### D5. Boot reseeds; it does not resume stored operator state

- Finding: the change ring is in memory, and `GraphCore::version` lives in `crates/medulla-graph-core/src/graph/core_cache.rs:47`, also in memory. A stored state has no durable position to resume from.
- Decision: R006 compiles and reseeds every plan view on boot.
- Alternative considered: resume from the stored state at a durable position. Rejected until a durable change position exists; resuming without one could skip changes.

### D6. Spill reuses the existing operator-state table

- Decision: spill writes whole-view state to `MATVIEW_OPERATOR_STATE` and reloads it by key.
- Rationale: the table is already registered in the owner registries and survives backup. A new table would need three registry entries.
- Alternative considered: per-key spill inside each operator. Rejected: every change would need a store read in the change path.

### D7. Demote, not refuse, when a view breaks a budget or a check

- Decision: a plan view that overflows, exceeds its ceiling or fails a shadow check moves to recompute mode with a typed code.
- Rationale: the caller keeps a correct answer. The code is visible in `medulla_catalog.matviews` and in metrics. This is a capability mode, not a legacy path.
- Alternative considered: refuse the read. Rejected: a read would fail for a condition the engine can serve correctly.

### D8. Exact types only for subtraction

- Decision: incremental `sum` and `avg` accept integers and decimals only. `min` and `max` accept floats because they need no subtraction.
- Rationale: float subtraction is not exact, so a maintained float sum drifts from a recompute.

### D9. Refusal codes

- Finding: the wire refusal vocabulary is the closed enum `ServerErrorCode` (`crates/medulla-types/src/contract/server_error_code.rs:8`). MEDULLA-ENGINE-PARITY-R014.1 and MEDULLA-FEDERATED-QUERY-R708.4 already add their codes there.
- Decision: every new code of this spec is a `ServerErrorCode` variant. The decision refusal taxonomy MEDULLA-DECISION-ENGINE-R010.100 covers decision outcomes only.

### D10. No new served method

- Decision: status is the system table `medulla_catalog.matviews`, read through SQL. View changes use `CdcRead` on feed key `view:<name>`.
- Rationale: a new method needs a new generated client contract in every language. Arbora already serves `Sql` and `CdcRead` as generated operations (ARBORA-OPS-R018).

### D11. UQL defines no views

- Finding: UQL is one read-only method (MEDULLA-FEDERATED-QUERY FQR-01).
- Decision: a plan view is defined by `PlanMatViewDefine` with the plan that UQL compiles to. UQL `EXPLAIN` reports maintainability (R001.8).

### D12. Stages not filed

- Decision: event-time lateness and state collection by lateness are not filed. No consumer runs unbounded stream joins today. Incremental graph algorithms are not filed either; they are warm restarts, not view maintenance.
