# MEDULLA-INCREMENTAL-VIEWS — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine already maintains plan views from change deltas. The kernel lives in `crates/medulla-exec/src/incremental/`, and `crates/medulla-server-cdc/src/matview/manager.rs` wires it to the change hub. R001 binds that shipped behavior to rows and adds a served proof. The plan then widens the delta to signed 64-bit weights (R002) and shares one oracle harness (R003, R004). It makes maintenance observable (R005), restores it on boot (R006), bounds and spills its state (R007) and checks it in production (R008). SQL views over one table are maintained inside the writing transaction (R009) once the SQL view catalog and row capture exist. View result changes ride the existing change feed (R010). The ontology closure gains delete-and-re-derive (R011). DISTINCT, min, max and one indexed equi-join follow (R012, R013). Reuse candidates found in the code: `Circuit`, `seed_delta_from_core`, `event_to_delta`, `matview::materialize`, `matview_operator_state_put`, `register_syscat_table`, the metrics pattern of `metrics/outbox.rs` and `Reasoner::add_axioms`.

## Technical Context

- **Language/Version**: Rust, toolchain of `rust-toolchain.toml`; Python 3.14 for the docs test.
- **Primary Dependencies**: `serde`, `rmp-serde`, `parking_lot`, `prometheus`, `redb`, `sqlparser` through `datafusion`, `proptest`, `tokio`.
- **Storage**: redb. Plan view definitions in the `plan_matviews` table; operator state and spilled state in the existing `MATVIEW_OPERATOR_STATE` table; SQL view rows in the result table of MEDULLA-ENGINE-PARITY-R014.1.
- **Testing**: unit (L1), integration and property oracles (L2), served dispatch and pgwire tests (L3), nightly benches (L6).
- **Target Platform**: the medulla server on Linux x86_64 and arm64.
- **Project Type**: service and library crates in one workspace.
- **Performance Goals**: 1,000-row plan delta at most 20 ms p99 at 1,000,000 rows; SQL commit-to-fresh at most 50 ms p99; view read at most 3 ms p99.
- **Constraints**: no second store, no new served method, no new feed, no float subtraction, every wait bounded.
- **Scale/Scope**: 72 rows; 12 bind shipped behavior; 60 are new.

## Constitution Check

- I. One owner per concept: pass. This spec owns maintenance; R014, R015 and R708 stay with their owners and are cited by ID.
- II. Extend before you create: pass. The search is recorded in spec.md "Why a new spec"; research.md records the rejected placement.
- III. Specs describe the current state only: pass. Shipped behavior is stated as current rows bound to existing tests.
- IV. Requirements are neutral and controlled: pass. No product names; sentences stay short.
- V. Hard cutover, no legacy path: pass. R002.1 moves the delta type with no re-export; R006 replaces the recompute-only reload.
- VI. Names carry no version suffix: pass.
- VII. One engine, no second database, no sync: pass. Spill reuses the existing redb table.
- VIII. Ask the graph, not a bespoke operation: pass. Status is a system table read through SQL; no new method.
- IX. Connectors depend on the SDK: not applicable.
- X. Every ontology class rolls up to the core: not applicable; no class is added.
- XI. Code stays small: pass. New code goes into new files; R005.4 moves `serve` out of `metrics.rs` to keep it under 900 lines.
- XII. No ratchets and no suppressions: pass. R007.3 removes an `#[allow(dead_code)]`.
- XIII. Zero flakes: pass. Served tests wait on bounded deadlines; property tests use fixed seeds in CI.
- XIV. Compile first: pass. Each row names files and types that compile in order.
- XV. Python 3.14 standard: pass for the docs test.
- XVI. Install and deploy from releases: not applicable.
- XVII. Publish the gate's tool first: pass. R004 is a test, not a new hook.
- XVIII. Landed means reachable: pass. Each rollup has a wiring row and a served proof.
- XIX. Tag push publishes: not applicable.
- XX. Secrets are never traced: pass. Logs carry view names and row ids only.
- XXI. Public files stay portable: pass. No host, private path or tool name.
- XXII. Names and placement: pass. Lower-case kebab-case names in the one place.
- XXIII. Versioning and release: pass. R015 states the release gate; no hand version edit.
- XXIV. Complete specifications: pass. Every row names caller chain, files, types, codes, knobs, writes, dependencies and test.
- XXV. Small, modular units: pass. Layering stays downward: `medulla-types` and `medulla-core` below the store, the SQL layer and the server crates.
- XXVI. Defects proved over the real protocol: pass. Served and pgwire tests prove each rollup.
- R-I. Native Rust, only fact store: pass.
- R-II. Every wait has a deadline: pass. R008.3 sets a 30 s timeout; R007.3 reads are bounded single-key reads.
- R-III. Budgets and a benchmark per row: pass. Every row states its latency and memory budget and names its benchmark.

## Project Structure

```text
crates/medulla-types/src/delta.rs                         new (R002.1)
crates/medulla-types/src/contract/server_error_code.rs    existing, new codes
crates/medulla-exec/src/incremental/zset.rs               existing, deleted (R002.1)
crates/medulla-exec/src/incremental/circuit.rs            existing (R002.3, R007.1, R010.1)
crates/medulla-exec/src/incremental/oracle.rs             new (R003)
crates/medulla-core/src/view_status.rs                    new (R005.1)
crates/medulla-kernel/src/metrics/views.rs                new (R005.4)
crates/medulla-kernel/src/metrics/serve.rs                new (R005.4)
crates/medulla-server-cdc/src/matview/manager.rs          existing (R002.3, R005.2, R007, R008.1, R010.1)
crates/medulla-server-cdc/src/matview/spill.rs            new (R007.3)
crates/medulla-server-cdc/src/cdc.rs                      existing (R010.1)
crates/medulla-h-txn/src/dist_compute.rs                  existing (R006)
crates/medulla-h-txn/src/matview_spill.rs                 new (R007.4)
crates/medulla-h-txn/src/matview_shadow.rs                new (R008.2, R008.3)
crates/medulla-h-admin/src/boot_maintenance.rs            existing (R007.4, R008.3)
crates/medulla-persistence/src/redb_backend/matview_api.rs existing (R007.4)
crates/medulla-sql/src/sql/catalog_matviews.rs            new (R005.3)
crates/medulla-sql-lang/src/sql/matview_compile.rs        new (R009.2, R012.3, R013.3)
crates/medulla-table-store/src/tables/matview_incremental.rs new (R009, R010.2, R012, R013)
crates/medulla-table-store/tests/matview_oracle.rs        new (R009.6)
crates/medulla-table-store/benches/matview_incremental.rs new (R009.8)
crates/medulla-plan/tests/incremental_oracle_coverage.rs  new (R004)
crates/medulla-plan/benches/incremental_delta.rs          new (R001.10)
crates/medulla-reasoner/src/owl.rs                        existing (R011.4, R011.5)
crates/medulla-reasoner/benches/owl_retract.rs            new (R011.7)
crates/medulla-server-cdc/src/reasoning_cascade.rs        existing (R011.6)
crates/medulla-server-served-tests/src/split_tests/incremental_views.rs new (served proofs)
tests/pgwire_roundtrip.rs                                 existing (pgwire proofs)
docs/interfaces/incremental-views.md                      new (R014)
tests/docs/test_incremental_views_doc.py                  new (R014)
```

## Phase 0: Research

Five unknowns were resolved in [research.md](research.md): whether SQL row-change events exist, how `CREATE MATERIALIZED VIEW` is stored, where SQL maintenance runs, whether boot may resume from stored operator state, and how state spills without a new table.

## Phase 1: Design

### Delta and circuit

R002 moves `ZRow` and `Delta` into `medulla-types` so the table store can use them. The weight widens to `i64`. `Circuit::apply` consolidates first and returns the output delta for the feed (R010.1). Types are in [data-model.md](data-model.md).

### Plan view lifecycle

Define compiles and seeds; a change folds; a read projects. Boot reseeds through the same `index_matview` (R006). The manager publishes every status change to the board of R005.1. The SQL layer renders the board as `medulla_catalog.matviews`.

### Budget and spill

The manager keeps resident state under `MEDULLA_VIEWS_MEMORY_BYTES` by spilling the least recently changed view to `MATVIEW_OPERATOR_STATE`. A view over `MEDULLA_VIEW_STATE_BYTES` demotes. Spilled bytes stay in memory until the store acknowledges them, so a reload never reads a missing row.

### Shadow verification

A background task recomputes due views off the change path. It compares only when the graph version did not move during the recompute. A mismatch demotes and is counted.

### SQL views

The SQL compiler turns the defining query into an `IncrementalViewProgram` or a named refusal. The store runs `maintain_views` inside the redb write transaction that applies the table ops. The view therefore commits with its inputs and is never stale. Grouped views store a hidden `__count`. A join probes the post-commit tables through their key indexes.

### Ontology closure

Support counts track how many justifications derive each subsumption. A retraction over-deletes the dependent region and re-derives what still has support.

### Failure modes

- Overflow: demote a plan view; refuse a SQL commit.
- Ceiling: demote with `STATE_BUDGET_EXCEEDED`.
- Mismatch: demote with `SHADOW_MISMATCH`.
- Store write failure during spill: the bytes stay pending in memory and the next spill retries.
- Restart: reseed every plan view from the graph.

The served behavior and its commands are in [quickstart.md](quickstart.md). The arbora seam is in [contracts/view-change-feed.md](contracts/view-change-feed.md).

## Phase 2: Task approach

`tasks.md` lists one task per row in dependency order. The current-state rows come first: their task adds the test binding and replaces the legacy concept marker. The first slice is the unblocked frontier: R001.10, R002, R003, R004, R005, R006, R007 and R008. R011.4 to R011.7 depend only on this spec and run in parallel. R009, R010, R012 and R013 start when MEDULLA-ENGINE-PARITY-R014.1, R014.2 and R015.1 land. Parallel tasks carry `[P]`.

## Complexity Tracking

none

## Build contract

Every change passes the hooks of `.config/pre-commit.yaml` at the pre-commit and pre-push stages, and the manual stage for the scanners below.

| Gate | Threshold | Command |
|---|---|---|
| Format | rustfmt clean | `cargo fmt --all -- --check` |
| Lint | zero warnings, full features | `cargo clippy --no-default-features --features full --all-targets -- -D warnings` |
| Complexity | no new or worsened function over cyclomatic 10 or cognitive 15 | `pre-commit run complexity-staged --hook-stage manual --config .config/pre-commit.yaml` |
| Clones | dupehound similarity 0.85 at 40 tokens; jscpd 50 tokens and 5 lines, zero new pairs | `pre-commit run dupehound-changed-functions jscpd-differential --hook-stage manual --config .config/pre-commit.yaml` |
| KISS (Rust) | statements per function 35; arguments 8; returns 8; indentation 5; locals 20; calls 45; methods per type 13; lines per file 900; functions per file 40; statements per file 250 | `pre-commit run kiss-changed-rust --hook-stage manual --config .config/pre-commit.yaml` |
| Import cycles | 0; dependency depth 4 | `pre-commit run rust-arch-lint --hook-stage manual --config .config/pre-commit.yaml` |
| Durable tables | no new table outside the registries | `pre-commit run durable-table-registration --config .config/pre-commit.yaml` |
| Affected tests | 120 s cap (organization pre-land fast set) | `cargo test --locked -p <each changed crate>` |
| Spec | clean | `pipelines-hook spec-standard`, `pipelines-hook plan-conformance` |
| Wording | 20 words per sentence | `pipelines-hook ste-staged` |

Code shapes: new logic goes into the new files listed above. No function takes more than 8 arguments. No new `#[allow]`. Every new code is a `ServerErrorCode` variant. Every knob is read once through the `ring_cap` pattern.

## Production caller chain

- Define: `Method::PlanMatViewDefine` arm (`crates/medulla-h-txn/src/dist_compute.rs:243`) → `define_plan_matview` (`dist_compute.rs:409`) → `index_matview` (`dist_compute.rs:448`) → `Circuit::compile` (`crates/medulla-exec/src/incremental/circuit.rs:253`) → `install_incremental` (`crates/medulla-server-cdc/src/matview/manager.rs:111`).
- Change: committed write → `CdcHub::emit` (`crates/medulla-server-cdc/src/cdc.rs:269`) → `crate::matview::apply_delta` (`cdc.rs:357`) → `PlanMatViewManager::apply_delta` (`manager.rs:172`) → `Circuit::apply` (`circuit.rs:287`).
- Read: `get_plan_matview` (`dist_compute.rs:499`) → `incremental_rows` (`manager.rs:144`).
- Boot: `spawn_matview_reload` (`crates/medulla-h-admin/src/boot_maintenance.rs:50`) → `reload_matviews` (`dist_compute.rs:661`) → the call at `dist_compute.rs:698` that R006 replaces.
- Explain: `Method::Uql` → `explain` (`crates/medulla-plan/src/uql/serve.rs:281`).
- SQL commit: pgwire `COMMIT` → `crates/medulla-h-query/src/query/sql_catalog.rs:267` → `commit_txn_batch` (`crates/medulla-table-store/src/tables/store.rs:2008`) → `commit_txn_batch_inner` (`store.rs:2081`) → `apply_mutation_txn_ops_with_crashpoints` (`store.rs:2267`).
- SQL create: `classify_create_view` (`crates/medulla-sql-lang/src/sql/classify.rs:2210`); today it refuses `CREATE MATERIALIZED VIEW` at `classify.rs:2216`.
- Feed read: `Method::CdcRead` arm (`crates/medulla-h-query/src/streaming.rs:429`).
- Closure: `reasoning_cascade::spawn` (`crates/medulla-server-cdc/src/reasoning_cascade.rs:358`) → `refresh_one` (`reasoning_cascade.rs:246`).

Obsolete paths to delete: `crates/medulla-exec/src/incremental/zset.rs` (R002.1) and the recompute-only reload at `dist_compute.rs:698` (R006).

## Cross-repo seams

- `ARBORA-OPS-R018` serves `CdcRead` on `view:<name>` feeds and the SQL entry as generated operations. Provider row: `MEDULLA-INCREMENTAL-VIEWS-R010.4`. Contract: [contracts/view-change-feed.md](contracts/view-change-feed.md).
