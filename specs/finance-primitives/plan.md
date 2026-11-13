# MEDULLA-FINANCE-PRIMITIVES — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

Medulla is the one authority for typed finance facts, fixed-point portfolio accounting, strategy evaluation, sealed backtest evidence and informational advice. Every served operation enters at the dispatch boundary of `crates/medulla-server-router`, passes `crates/medulla-server-graph` and reaches one of two handler paths. Stateless numeric and market calculations run in `crates/medulla-h-compute`. Record, accounting, alert and proposal operations that need the graph core run in `crates/medulla-h-graph`. Shared kernels live once in `crates/medulla-numeric`; the finance crates are callers. The work extends existing owners first: `finance-v1` ontology and shapes, `crates/medulla-finance-market` for `BacktestRun` and `AnalysisSnapshot`, `crates/medulla-finance` for lot accounting and `crates/medulla-finance-quant` for factor, leverage and execution kernels.

```text
arbora (admission, auth, principal) --> medulla dispatch
  FinanceMarket / NumericEvaluate / FinanceRecordPut,Get / FinanceAlertRaise / FinanceOrderStage,Promote
    stateless  --> medulla-h-compute  finance/routes.rs --> medulla-finance-market, -quant, medulla-numeric
    graph core --> medulla-h-graph    gateway route FinanceRecords / terminal.rs read chain
                      --> medulla-finance, SHACL validation, MutationBatch, outbox
```

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Python 3.14 (floor 3.12) for served client tests.
- **Primary Dependencies**: The workspace crates named in Project Structure and the standard library. No broker, market or model service.
- **Storage**: The engine graph store holds immutable source records and sealed results through the mutation gateway. Derived series state uses the existing derived-state commit. The finance outbox holds event intents.
- **Testing**: Rust unit tests per crate, served Rust tests under `tests/` through signed dispatch, and `tests/test_finance_record_client.py` for the client.
- **Target Platform**: Linux x86_64 and aarch64 hosts without AVX2 assumptions; the pinned soft-float kernel and fixed-point money keep digests bit-identical.
- **Project Type**: Library crates and server handler crates.
- **Performance Goals**: Each served kernel names its budget in its row; the hot-path rows (`MEDULLA-FINANCE-PRIMITIVES-R002.10`, `MEDULLA-FINANCE-PRIMITIVES-R613.6`, `MEDULLA-FINANCE-PRIMITIVES-R018.6`) hold the latency measurements.
- **Constraints**: Tenant visibility precedes every read or count. No binary float in a persisted monetary result. Each refusal has one stable code. No version suffix in a name. No second state store.
- **Scale/Scope**: The 164 leaf rows of `requirements.md` over 6 user stories and 15 `FR-` labels.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate and the engine store.
- **R-II. Every wait has a deadline**: pass. Root searches, filters and simulations carry explicit iteration or step bounds.
- **R-III. Every row sets budgets and names a benchmark**: pass. Hot-path rows hold the budgets.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; `medulla-h-graph` calls the kernel crate `medulla-decision-outcome-types`, not a sibling handler crate; retired `Finance<Kernel>` methods are deleted in the same release.
- **Organization standards**: pass. Rows follow the spec standard and ecosystem boundaries.

## Project Structure

```text
crates/medulla-core/ontology/finance-v1.ttl, finance-v1.shapes.ttl   existing: classes and shapes, extended
crates/medulla-types/src/wire_finance.rs                              existing: record kinds, policy, version gate
crates/medulla-types/src/numeric_kernel.rs                            new: NumericEvaluateOp, output, refusal
crates/medulla-types/src/finance_event_envelope.rs                    existing: alert envelope
crates/medulla-numeric/src/evaluation/, series/, calibration/         existing and new: shared kernels
crates/medulla-finance/src/                                           existing: lot accounting, import, golden fixtures
crates/medulla-finance-market/src/market/                             existing: BacktestRun, AnalysisSnapshot, digest
crates/medulla-finance-quant/src/                                     existing: factor, leverage, execution kernels
crates/medulla-h-compute/src/finance/, numeric.rs                     existing and new: stateless handlers
crates/medulla-h-graph/src/graph_ops/finance_records.rs               new: record, factor and import handlers
crates/medulla-h-graph/src/graph_ops/finance_accounting.rs            new: projection handler
crates/medulla-h-graph/src/graph_ops/finance_alerts.rs                new: alert transaction handler
crates/medulla-h-graph/src/graph_ops/finance_order_proposal.rs        new: stage and promote
crates/medulla-protocol-types/src/protocol/method/                    existing: Method variants
crates/medulla-server-router/src/dispatch/router.rs                   existing: dispatch
tests/                                                                existing and new: served tests
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns; no research.md.

## Phase 1: Design

The rules below bind every row.

1. **One money rule.** Money, quantities, rates and FX use checked fixed-point with explicit scale and rounding. No binary float enters a persisted monetary result. Statistical kernels may use controlled numeric code, but monetary inputs and outputs stay fixed-point.
2. **Append-only source facts.** An activity or reference revision carries an effective time and a `known_at` time. A correction supersedes a prior version; replay at an earlier `known_at` sees the prior one. The activity journal is authoritative for holdings and a `Position` is a discardable projection.
3. **One kernel authority.** `crates/medulla-numeric` holds each generic kernel once. Callers reach it through `NumericEvaluate`, UQL `DERIVE`, `SKILL` and `EVALUATE`, or a derived series. A finance wrapper over a kernel is deleted.
4. **Served proof.** A behaviour row is VERIFIED only by a test that enters through signed dispatch with a tenant and principal context. A unit test alone proves a shape, not a row.
5. **Gateway route for records.** `FinanceRecordPut`, `FinanceRecordGet`, `FinanceFactorRecord`, the tracker import and the alert, receipt and proposal methods are gateway-routed through the `FinanceRecords` route into the handler modules of `crates/medulla-h-graph/src/graph_ops/`; the stateless `crates/medulla-h-compute/src/finance/routes.rs` has no graph core and takes none of them.
6. **Sealed evidence.** `BacktestRun`, `AnalysisSnapshot` and `OrderProposalSnapshot` are sealed classes; a generic node write cannot overwrite them.
7. **Trust boundary.** A recommendation and a `ProposedAction` are data. The protocol has no generic execute conversion, no broker credential and no write-back to an external source. Live leveraged orders need a per-instrument policy and designated approvers.
8. **Wire changes.** The Rust serde types are the only wire source. A wire change names the changed type and its served test. An unknown version fails closed.
9. **Owners.** A medulla row keeps the fact, record and calculation side. Served, client and connector parts live in the owner's spec and link by ID.

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row in table order, producer first. A record or kernel type precedes the rows that wire it. A row lands in one slice with its caller, so no type lands without a production caller. Tasks of different crates run in parallel; tasks that edit the same `Method` enum file serialize.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Each hot-path row measures its latency budget on the reference host.
- A public item in a finance crate has a non-test caller under a handler or server crate. `MEDULLA-CONTRACT-R055` gates this.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Dispatch stays in `crates/medulla-server-router`; a handler crate never calls a sibling handler crate.

## Production caller chain

- `Method::FinanceMarket` (signed dispatch, `crates/medulla-server-router/src/dispatch/router.rs::dispatch_request_method_inner`) to `dispatch_graph_op` (`crates/medulla-server-graph/src/dispatch/graph_pipeline.rs`) to `crates/medulla-h-compute/src/finance.rs::try_handle` to `finance/routes.rs::try_handle` to `finance/market.rs::handle_market` to `crates/medulla-finance-market/src/market/backtest_run.rs::seal` and the indicator, strategy and snapshot code.
- `Method::NumericEvaluate` to `crates/medulla-h-compute/src/numeric.rs` (new) to `crates/medulla-numeric/src/evaluation/registry.rs::evaluate` (new).
- `Method::FinanceRecordPut`, `FinanceRecordGet` and `FinanceFactorRecord` to `crates/medulla-server-mutation/src/mutation/plan.rs::is_gateway_routed` to `crates/medulla-h-graph/src/graph_ops/gateway.rs::try_handle_gateway` to `gateway_graph_routes.rs` (`Route::FinanceRecords`, new) to `crates/medulla-h-graph/src/graph_ops/finance_records.rs` (new).
- Position and regime reads to `crates/medulla-h-graph/src/graph_ops/terminal.rs` (`try_handle_finance` arm, new) to `finance_accounting.rs::handle_project` and `finance_regime.rs::handle_regime_forecast` (new) to `crates/medulla-finance/src/lot_accounting.rs::apply_activities` and `crates/medulla-numeric/src/series/hmm.rs`.
- `FinanceAlertRaise` to the gateway route to `finance_alerts.rs::raise` (new) to the owner transaction and outbox.
- `FinanceOrderStage` and `FinanceOrderPromote` to the gateway route to `finance_order_proposal.rs` (new) to the sealed-record guard in `crates/medulla-mutation-types/src/sealed_record.rs`.
- `TsAppend` to `crates/medulla-h-txn/src/timeseries/derived.rs::maintain_one` to `crates/medulla-tsdb/src/derive/maintain.rs::DerivedState::advance` for the state-space models.
- Obsolete paths to delete: the 14 `Finance<Kernel>` methods, their route arms, capability lines and the wrappers in `crates/medulla-finance-market`, once `NumericEvaluate` lands.

## Cross-repo seams

- Market operations over finance records, positions and strategies: `MEDULLA-FINANCE-PRIMITIVES-R004.3.2` provides, `ARBORA-DATA-MARKET-R003` consumes. Contract: [arbora-market-operations.md](contracts/arbora-market-operations.md).
- Numeric kernel evaluation for connector analyses: `MEDULLA-FINANCE-PRIMITIVES-R002.2.5` provides, `SCION-FINANCE-SOURCES-R902` and `EMERALD-OPS-R007` consume. Contract: [numeric-evaluate-clients.md](contracts/numeric-evaluate-clients.md).
- Order proposal staging and promotion: `MEDULLA-FINANCE-PRIMITIVES-R015.1` provides, `ARBORA-DATA-MARKET-R007` and `SCION-GOVERNED-WRITEBACK-R001` consume. Contract: [order-proposal-admission.md](contracts/order-proposal-admission.md).
- Banking risk calculations: `MEDULLA-FINANCE-PRIMITIVES-R901` provides, `ARBORA-DATA-MARKET-R003.14`, `SCION-FINANCE-SOURCES-R904` and `EMERALD-OPS-R013` consume. Contract: [banking-risk-operations.md](contracts/banking-risk-operations.md).

- Forecast samples (R902): `ForecastSample` is ingested through `SourceIngest` and stored with `probabilistic = true`. `FinancePositionSize.series_revision` and a backtest draft's data revisions are checked by `finance_forecast_gate::refuse_probabilistic_input` before the pure kernels; a forecast revision is refused with `PROBABILISTIC_INPUT_REFUSED`.
