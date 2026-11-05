# MEDULLA-FINANCE-PRIMITIVES — Finance primitives: accounts, strategies, risk and evidence

**Owner:** Medulla (EG) · **Program:** finance asset manager

## Outcome and boundary

Medulla is the authority for typed finance facts, deterministic portfolio calculations, strategy evaluation, backtest evidence, and informational recommendations. A client can import dated activities, reconstruct a portfolio at any past knowledge time, compare strategies, and inspect every displayed number's source and as-of time. The engine emits proposals and analysis, never an executable order or an approval lease. Connectors own external acquisition; the serving layer owns schedules and delivery; a browser owns presentation. Each can use the public Medulla contracts specified here without access to any other documentation.

Existing `finance-v1` already models `FinancialInstrument`, `Listing`, `Venue`, `BarSeries`, `IndicatorSpec`, `SignalState`, `TrendFlip`, `BacktestRun` and `AnalysisSnapshot`. Existing `FinanceMarket` operations resolve revisions, compute indicators, seal backtests and snapshots, and publish `finance.flip` events. Extend these types and operations; keep bars in the time-series store rather than copying bars into graph nodes. Reuse the engine's sealed-record guard and graph authorization for persisted account and strategy records.

The `BacktestRun` content digest must be canonical across supported hosts: normalize serialization, floating-point and timestamp inputs before hashing, and compare independent builds on two architectures. The SuperTrend/ATR reference companion in Pine must produce the same signal transitions as Medulla from the same checked-in bar fixture and parameter version. Fold finance-specific classes into the one `finance-v1` ontology and remove duplicate class definitions; this obligation is tracked as MEDULLA-FINANCE-PRIMITIVES-R001.

## Scope

Owner: `medulla` (native Rust). The finance crates are `crates/medulla-finance`, `crates/medulla-finance-market`, `crates/medulla-finance-quant`, `crates/medulla-numeric`, `crates/medulla-h-compute`, `crates/medulla-h-graph`, `crates/medulla-types` and `crates/medulla-tsdb`. Dispatch stays in `crates/medulla-server-router` and `crates/medulla-server-graph`.

Non-goals: the engine never places an order, never holds a broker credential and never writes back to an external portfolio source. Arbora owns admission and the served door. Connector repositories own their source calls. A recommendation or `ProposedAction` is data, not an effect.

Why one spec: accounts, strategies, risk and evidence share one set of keys, one fixed-point money rule and one provenance rule. No other spec owns them.

## User Scenarios & Testing

### User Story 1 - Keep typed, versioned finance records (Priority: P1)

The user is an operator who records accounts, activities, lots and strategies. The outcome is a validated record store whose shapes refuse unknown versions, and the reason is that every later figure rests on these records.

**Acceptance scenarios**

1. **Given** an operator submits a valid `Account` through `FinanceRecordPut`, **When** the served handler validates the SHACL shape and upserts under the verified tenant, **Then** `FinanceRecordGet` returns the same fields.
2. **Given** a record declares an unknown `finance-v1` schema version, **When** it reaches the wire boundary, **Then** the request fails with `FINANCE_RECORD_VERSION_UNKNOWN` and writes nothing.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R004.3.2` run this story through signed dispatch.

### User Story 2 - Reconstruct a portfolio from dated activities (Priority: P1)

The user is an analyst who needs holdings, cost basis and returns at a past knowledge time. The outcome is a fixed-point projection with provenance, and the reason is that a figure must be reproducible.

**Acceptance scenarios**

1. **Given** dated activities and a corporate action, **When** the engine projects the account as of a time, **Then** only revisions with `known_at` at or before that time apply and lots follow the elected basis method.
2. **Given** a figure lacks source, as-of or currency provenance, **When** it is compared with a golden fixture, **Then** the comparison is refused.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R005.1` and `MEDULLA-FINANCE-PRIMITIVES-R013.2.6` run this story.

### User Story 3 - Evaluate strategies and seal backtest evidence (Priority: P1)

The user is a researcher comparing dollar-cost averaging, lump sum and trend rules. The outcome is a sealed `BacktestRun` with a canonical digest, and the reason is that results must be verifiable on any host.

**Acceptance scenarios**

1. **Given** a `StrategySpec` and point-in-time bars, **When** the evaluator runs and the run is sealed, **Then** the digest is identical on two supported architectures.
2. **Given** a sealed run carries split results, **When** the probability of backtest overfitting is computed, **Then** it derives from the run's own split matrix and not a caller-provided summary.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R001` and `MEDULLA-FINANCE-PRIMITIVES-R003` run this story.

### User Story 4 - Run shared numeric kernels and streaming state models (Priority: P2)

The user is a caller of numeric evaluation, UQL or a derived series. The outcome is one kernel authority reached through `NumericEvaluate`, `DERIVE`, `SKILL` and `EVALUATE`, and the reason is that no caller keeps a private copy.

**Acceptance scenarios**

1. **Given** a kernel such as the rolling z-score, **When** a caller reaches it through `NumericEvaluate` or UQL, **Then** both paths return the same value as the independent oracle.
2. **Given** a retired `Finance<Kernel>` method, **When** a client sends it, **Then** the request fails to decode.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R002.2.5` run this story.

### User Story 5 - Advise within risk limits and stage order proposals (Priority: P2)

The user is an investor or approver who receives informational advice. The outcome is a calibrated recommendation or an abstention, a margin simulation and a sealed order proposal, and the reason is that no live action happens without policy and approval.

**Acceptance scenarios**

1. **Given** a holding with a sealed backtest and a calibration cohort, **When** the engine recommends, **Then** the result cites its evidence or abstains.
2. **Given** a live leveraged order without a matching per-instrument policy, **When** eligibility is checked, **Then** it is refused.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R009.1` and `MEDULLA-FINANCE-PRIMITIVES-R010.1` run this story.

### User Story 6 - Raise finance events and import from a portfolio tracker (Priority: P3)

The user is an operator of downstream delivery and of an optional attached source. The outcome is stable finance events and an idempotent one-way import, and the reason is that delivery and re-import must not duplicate.

**Acceptance scenarios**

1. **Given** a threshold is crossed, **When** `FinanceAlertRaise` commits, **Then** one outbox intent with a stable event ID is written atomically with the fact.
2. **Given** a portfolio-tracker export is imported twice, **When** the second run executes, **Then** it writes nothing new and a write-back direction is refused.

Independent test: the bound tests of `MEDULLA-FINANCE-PRIMITIVES-R011.2.2` and `MEDULLA-FINANCE-PRIMITIVES-R012.2` run this story.

### Edge Cases

- A tenant other than the verified one is refused with `ACCESS_DENIED` and nothing is counted.
- A missing price, FX rate or scale yields a typed partial or refusal and never a substituted value.
- A binary float never enters a persisted monetary result.
- A correction supersedes a prior revision while replay at an earlier `known_at` sees the prior one.
- A stale expected head returns `RECORD_HEAD_CONFLICT`.
- A recommendation without calibration evidence abstains.
- A live order never follows from a `ProposedAction` inside the engine.

## Requirements

- **FR-001**: Typed, versioned finance records with shape validation. Rows: `MEDULLA-FINANCE-PRIMITIVES-R004`, `MEDULLA-FINANCE-PRIMITIVES-R004.3.2`, `MEDULLA-FINANCE-PRIMITIVES-R014`.
- **FR-002**: Fixed-point lot accounting and returns. Rows: `MEDULLA-FINANCE-PRIMITIVES-R005`, `MEDULLA-FINANCE-PRIMITIVES-R005.1`, `MEDULLA-FINANCE-PRIMITIVES-R005.2`.
- **FR-003**: Point-in-time corporate actions and sessions. Rows: `MEDULLA-FINANCE-PRIMITIVES-R006`.
- **FR-004**: Immutable strategy specs and proposals. Rows: `MEDULLA-FINANCE-PRIMITIVES-R007`, `MEDULLA-FINANCE-PRIMITIVES-R007.1`.
- **FR-005**: Sealed backtests with canonical digest and split-derived overfit probability. Rows: `MEDULLA-FINANCE-PRIMITIVES-R001`, `MEDULLA-FINANCE-PRIMITIVES-R003`, `MEDULLA-FINANCE-PRIMITIVES-R008`.
- **FR-006**: One shared numeric kernel authority. Rows: `MEDULLA-FINANCE-PRIMITIVES-R002`, `MEDULLA-FINANCE-PRIMITIVES-R002.2.5`, `MEDULLA-FINANCE-PRIMITIVES-R002.2.4`.
- **FR-007**: Causal state-space filtering on the hot paths. Rows: `MEDULLA-FINANCE-PRIMITIVES-R002.5`, `MEDULLA-FINANCE-PRIMITIVES-R002.6`, `MEDULLA-FINANCE-PRIMITIVES-R002.10`.
- **FR-008**: Calibrated, abstaining recommendations. Rows: `MEDULLA-FINANCE-PRIMITIVES-R009`, `MEDULLA-FINANCE-PRIMITIVES-R009.1`, `MEDULLA-FINANCE-PRIMITIVES-R902`.
- **FR-009**: Margin risk and the live-order boundary. Rows: `MEDULLA-FINANCE-PRIMITIVES-R010`, `MEDULLA-FINANCE-PRIMITIVES-R010.1`, `MEDULLA-FINANCE-PRIMITIVES-R010.2`.
- **FR-010**: Finance events through the outbox. Rows: `MEDULLA-FINANCE-PRIMITIVES-R011`, `MEDULLA-FINANCE-PRIMITIVES-R011.2`, `MEDULLA-FINANCE-PRIMITIVES-R011.3`.
- **FR-011**: One-way optional portfolio import. Rows: `MEDULLA-FINANCE-PRIMITIVES-R012`, `MEDULLA-FINANCE-PRIMITIVES-R012.1`, `MEDULLA-FINANCE-PRIMITIVES-R012.2`.
- **FR-012**: Golden fixtures and provenance on every figure. Rows: `MEDULLA-FINANCE-PRIMITIVES-R013`, `MEDULLA-FINANCE-PRIMITIVES-R013.2`, `MEDULLA-FINANCE-PRIMITIVES-R017`.
- **FR-013**: Factor, pattern and strategy research records. Rows: `MEDULLA-FINANCE-PRIMITIVES-R018`, `MEDULLA-FINANCE-PRIMITIVES-R750`, `MEDULLA-FINANCE-PRIMITIVES-R751`.
- **FR-014**: Immutable order proposals with an atomic admission boundary. Rows: `MEDULLA-FINANCE-PRIMITIVES-R015.1`, `MEDULLA-FINANCE-PRIMITIVES-R015`.
- **FR-015**: Sizing, distribution-shift and banking risk kernels. Rows: `MEDULLA-FINANCE-PRIMITIVES-R900`, `MEDULLA-FINANCE-PRIMITIVES-R901`, `MEDULLA-FINANCE-PRIMITIVES-R613`.

## Success Criteria

- **SC-001**: A `BacktestRun` digest matches on two supported architectures. Rows: `MEDULLA-FINANCE-PRIMITIVES-R001`.
- **SC-002**: A projection at a past `known_at` reproduces the figures of the independent statement vectors. Rows: `MEDULLA-FINANCE-PRIMITIVES-R005`, `MEDULLA-FINANCE-PRIMITIVES-R013.2`.
- **SC-003**: Each shared kernel equals its independent oracle through every served path. Rows: `MEDULLA-FINANCE-PRIMITIVES-R002.1`, `MEDULLA-FINANCE-PRIMITIVES-R002.2`.
- **SC-004**: No retired `Finance<Kernel>` method remains in `Method`. Rows: `MEDULLA-FINANCE-PRIMITIVES-R002.2.2`.
- **SC-005**: A re-import of the same source rows writes zero new records. Rows: `MEDULLA-FINANCE-PRIMITIVES-R012.2`.
- **SC-006**: A finance event commits with its outbox intent in one transaction. Rows: `MEDULLA-FINANCE-PRIMITIVES-R011.2`.
- **SC-007**: No path from a recommendation reaches a live order without policy and approval. Rows: `MEDULLA-FINANCE-PRIMITIVES-R010.1`, `MEDULLA-FINANCE-PRIMITIVES-R015.1`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- Fixtures are synthetic or published reference vectors; no live broker or market account is required.
- Calibration thresholds, sample floors and state weights come from versioned policy facts; the engine sets no default.

## Behavior detail

### Evidence and obligations

Delivery state is computed from the rows and their bound tests; it is never written.
Record exact commits, commands, hosted runs and scenario outcomes as evidence under `evidence/`.
Existing components establish reuse opportunities, not completion of these obligations.

| ID | Medulla obligation |
|---|---|
| MEDULLA-FINANCE-PRIMITIVES-R001 | Cross-host deterministic `BacktestRun` digest, Pine companion parity for SuperTrend/ATR, and one canonical `finance-v1` class authority. |
| MEDULLA-FINANCE-PRIMITIVES-R004 | Extend ontology and validated portfolio record types. |
| MEDULLA-FINANCE-PRIMITIVES-R005 | Fixed-point lot accounting and performance kernels. |
| MEDULLA-FINANCE-PRIMITIVES-R006 | Point-in-time corporate actions and market sessions. |
| MEDULLA-FINANCE-PRIMITIVES-R007 | Versioned typed strategy evaluator and proposal records. |
| MEDULLA-FINANCE-PRIMITIVES-R008 | Strategy comparisons through sealed backtests. |
| MEDULLA-FINANCE-PRIMITIVES-R009 | Calibrated, abstaining informational recommendations. |
| MEDULLA-FINANCE-PRIMITIVES-R010 | Leverage risk models and live-action refusal boundary. |
| MEDULLA-FINANCE-PRIMITIVES-R011 | Finance event facts for downstream delivery. |
| MEDULLA-FINANCE-PRIMITIVES-R012 | Optional one-way portfolio import from an attached source. |
| MEDULLA-FINANCE-PRIMITIVES-R013 | Golden accounting/risk fixtures and provenance invariants. |
| MEDULLA-FINANCE-PRIMITIVES-R002 | Shared numeric kernel ownership, thin compatibility aliases, UQL and method-count reconciliation. |
| MEDULLA-FINANCE-PRIMITIVES-R003 | Split-derived PBO and exact-median convention in sealed backtests. |

### Normative requirements

#### F1. Identity and records (MEDULLA-FINANCE-PRIMITIVES-R004)

1. Asset class is a validated enum: `equity`, `etf`, `fund`, `index`, `fx_pair`, `commodity_spot`, `commodity_future`, `crypto`, `real_estate`, `cash`; preserve mappings from current `stock`, `forex`, and `commodity` values during migration. A tradable identity remains `(instrument, venue, quote instrument, listing type)`, not a ticker alone. An index and manually valued real estate are observational holdings; they are not order targets.
2. `Account` has tenant, owner, base currency, account kind, source identity and policy scope. `Activity` has stable source ID, account, instrument/listing where relevant, type (`buy`, `sell`, `dividend`, `split`, `fee`, `transfer`, `interest`, `deposit`, `withdrawal`), effective time, known-at time, quantity, currency amounts, source digest and revision. Its identity is `(tenant, account, source, source activity ID, revision)`. Same revision plus different content is a conflict. Corrections append a higher revision.
3. `Lot` is derived from activities and records acquisition time, remaining quantity, native-currency cost and source activity. `Position` is a dated projection with quantity, valuation source, as-of time and currency; neither is an independently writable accounting authority. `PortfolioGroup` associates holdings or watchlist items with named user views; group membership never changes accounting.
4. `StrategySpec`, `StrategyRun`, `LeverageTerms`, corporate actions and valuation records have schema versions, tenant/visibility, source, effective time and known-at time. Unknown schema versions fail closed. OWL classes and SHACL shapes enforce required links, cardinality and controlled vocabularies; public method contracts carry the same invariants.

#### F2. Deterministic accounting (MEDULLA-FINANCE-PRIMITIVES-R005)

1. Money, quantities, rates and FX conversions use checked integer/decimal arithmetic with explicit scale, rounding mode and currency minor-unit metadata. No binary float can enter a persisted monetary result. Overflow and missing scale are typed errors. Replaying identical ordered inputs yields identical bytes.
2. On an as-of query, include only activity/corporate-action revisions whose `known_at <= as_of`, then order by effective time, source ID and revision. Support FIFO, LIFO, specific-lot and average-cost elections; record the election per account and tax context. A sell that exceeds available lots, invalid specific-lot allocation, duplicate event, unsupported action or unresolved split fails with a typed reason rather than silently creating inventory.
3. Realized P&L derives from disposed cost and proceeds less allocated fees. Unrealized P&L uses a sourced valuation at its own as-of time. Dividends, transfers and corporate actions preserve economic continuity; splits rescale quantity and per-unit basis without changing total basis. Trade-date FX is used for booked cash flows, valuation-date FX for current base-currency value, and the exact rate record is cited.
4. Time-weighted return geometrically chains subperiod returns separated by external cash flows. Money-weighted return solves dated cash flows as XIRR with a bounded deterministic root search and reports no unique root where appropriate. Benchmark comparison uses the same time window and explicit currency/return basis. Missing prices, stale valuations or unknown corporate actions yield partial results with coverage metadata, never a fabricated zero.

#### F3. Temporal reference data (MEDULLA-FINANCE-PRIMITIVES-R006)

Corporate actions (`split`, `dividend`, `symbol_change`, `delisting`) identify the affected immutable listing/instrument, effective date, announcement/known-at time, source and revision. A renamed ticker remains linked to the same economic identity unless a source explicitly identifies a successor. Calendars are versioned data containing regular, pre-market and post-market spans, holidays, early closes and time-zone offset spans. Every price/valuation result declares `session` (`regular`, `pre`, `post`, `closed`, `unknown`), source revision and as-of/known-at time. Do not query the host clock or ambient time-zone database in replay kernels.

#### F4. Strategies and comparisons (MEDULLA-FINANCE-PRIMITIVES-R007, MEDULLA-FINANCE-PRIMITIVES-R008)

`StrategySpec` is immutable by `(id, version, canonical digest)` and binds universe, currency, calendar, decision cadence, parameters, cost model and risk policy. Evaluators consume point-in-time bars/positions and produce `ProposedAction` with target, direction, size/rationale, input digests, decision time and expiry. No order transport, broker credentials or approval token are present in the result. Built-ins: DCA by fixed cash or shares and value-averaging target; existing SuperTrend flips and 50/200 moving-average trend; calendar and threshold rebalance. A missed cadence emits `MISSED` evidence, and retry idempotency prevents double proposals.

Compare DCA, lump sum and trend on the same point-in-time universe, cash flows, calendar, costs and valuation horizon. Reuse `BacktestRun` sealing and no-look-ahead validation. The engine computes purged combinatorial cross-validation, deflated Sharpe and probability of backtest overfitting from the *same* underlying split results, not caller-supplied headline metrics. Reject short histories, overlapping train/test information, missing transaction costs or incompatible runs. A comparison includes run digests, assumptions and uncertainty; it makes no universal performance claim.

#### F5. Recommendation and risk (MEDULLA-FINANCE-PRIMITIVES-R009, MEDULLA-FINANCE-PRIMITIVES-R010)

For each holding/watchlist item and horizon, a recommendation is `accumulate`, `hold`, `de_risk` or `abstain`. A non-abstaining result cites the strategy ID/version, sealed backtest/evaluation record, calibration cohort, data revision, price source and as-of time. Insufficient history, warmup, drift, ambiguity, stale price, unsupported jurisdiction or missing risk terms must abstain with a typed reason. Scorecards compare prior recommendations to later outcomes using an immutable evaluation window. Seal informational output as `AnalysisSnapshot`; no output authorizes an order.

`LeverageTerms` identifies instrument, product type (`margin`, `future`, `daily_reset_fund`, `fx`), jurisdiction/policy version, multiplier, tick, currency, initial and maintenance margin, financing/fee schedule, expiry/roll and liquidation convention as applicable. Simulate margin calls, liquidation levels, sizing and risk-of-ruin under explicit scenarios. Daily-reset funds compound daily returns, not a horizon return multiplied by leverage. Unknown terms or unavailable product policy refuse any live-action eligibility. Medulla's default is paper analysis. A separate authorized execution system must enforce per-instrument policy, a valid approval lease, risk guards and authorized approver; Medulla cannot mint or bypass any of these.

#### F6. Events and optional import (MEDULLA-FINANCE-PRIMITIVES-R011, MEDULLA-FINANCE-PRIMITIVES-R012)

Finance events use a versioned envelope `(tenant, event_id, topic, subject, occurrence_time, known_at, source_digest, payload_version)`. `finance.flip` keeps its existing stable ID. Add `finance.price`, `finance.dca_due` and `finance.margin_threshold` facts as needed; replay and outbox publication are idempotent. Delivery channels and scheduling remain consumers. An event never authorizes a trade.

An optional external portfolio source can attach read-only through Medulla's typed source registry. A one-way mapper converts source activities into F1 records with original source key, source revision and digest. Re-import is idempotent, deletions become explicit tombstone/correction records, and source schema drift rejects or quarantines records. The external database retains its own schema and migrations. Medulla never writes into it and never shares its storage ownership. This optional connector is independent of completing F1–F5.

#### F7. Quality and provenance (MEDULLA-FINANCE-PRIMITIVES-R013)

Every externally displayed monetary, price, return, risk or recommendation field carries source ID/digest, economic as-of, known-at, session where market-derived, currency and precision. The engine must pass broker-statement, corporate-action, DST/session, futures-roll and daily-reset-fund golden cases. A trace can reconstruct each number from input revisions and formula version. CCCC, jscpd, Dupehound and KISS checks apply to changed code; no duplicate accounting or finance decision authority is introduced in another layer.

### Completion contract

#### Shared evaluation kernels and split-derived overfit probability (MEDULLA-FINANCE-PRIMITIVES-R002, MEDULLA-FINANCE-PRIMITIVES-R003)

**MEDULLA-FINANCE-PRIMITIVES-R002.** Generic rolling z-score, EWMA, information coefficient/ratio, effective sample size, Diebold–Mariano, deflated Sharpe, purged combinatorial cross-validation, probability of backtest overfitting, Brier, Kalman, Ornstein–Uhlenbeck, ADF and regime HMM calculations have one domain-neutral owner in `medulla-numeric` evaluation/calibration. Time-series UQL operators call those kernels through the existing series execution path. Finance methods may remain only as thin, documented compatibility aliases while public callers migrate to the typed generic surface; no copied implementation, silent shim or second precision/rounding convention is allowed. Reconcile generated method counts and client bindings from the canonical method inventory in one change. A regime derivation requiring an unbounded prefix refit must declare a budget or refuse with typed state-budget failure; it cannot report a bounded success it did not compute.

**MEDULLA-FINANCE-PRIMITIVES-R003.** A `BacktestRun` computes PBO from its own combinatorial symmetric cross-validation split matrix, not a separate caller-provided summary row array. For each split, select the highest in-sample candidate by a deterministic tie rule, rank its out-of-sample performance against all candidates, and count it as overfit only when strictly below the exact median; equality to median is not overfit. Record split count, selected candidate, rank and convention in verifiable evidence. A missing/degenerate matrix, unequal candidate dimensions, nonfinite values or mismatched reported split count refuses sealing. The same split matrix supplies related validation outputs so a headline PBO cannot contradict the sealed record.

Acceptance requires exact-head hosted checks and deterministic positive and negative fixtures.
It also requires schema generation, authorization, replay and the applicable quality gates in test-spec.md.
Baseline checks use disposable data without private credentials.
Optional external-source integration requires disposable fixtures.
Record deployment qualification separately from source evidence.

Requirement IDs are defined in [requirements.md](requirements.md); delivery state per ID is computed.


R002 also owns versioned causal state-space filtering, bounded innovation retuning and robust nonlinear approximation.
Its original kernel inventory and acceptance remain mandatory.
The plan defines all four state models, production attachment points and prerequisite contracts.
The test contract requires independent numerical oracles and real served positive and negative sequences.

R002.1, R002.2 and R002.6 retain their acceptance as rollups of kernel-specific and model-specific children.
The linear recurrence requires mutually uncorrelated process and measurement noises; unsupported correlation declarations refuse.

### Order proposals and regime forecasts

#### `MEDULLA-FINANCE-PRIMITIVES-R015.1`

**Medulla-owned lifecycle.** Medulla defines `FinanceOrderProposal.StageV1` and `PromoteV1`. `OrderProposalSnapshotV1` is sealed and content-addressed. It binds tenant/proposal/revision, predecessor digest, decision identity/revision/digest, and schema identity/revision. It also binds canonical payload bytes/SHA-256, policy identity/revision/digest, and evidence refs. Hash stored bytes as-is. Refuse noncanonical bytes for the declared schema/version. Staging is create-only. Revision 1 has no predecessor. Later revisions name the prior digest and use expected-head CAS. An identical retry is idempotent; conflicting bytes refuse.

`PromoteV1` binds the expected promoted head, candidate digest, gate-manifest digest, and gate receipts. It rechecks the staged payload hash/digest. It rechecks R019 verification/approval and the R010 risk/sizing policy. Apply per-instrument policy and designated approvers when required. Atomically write the promotion pointer, `derivedFrom` history, and `OrderAdmissionReceiptV1`. Bind the receipt to proposal, revision, snapshot, decision, policy, gate receipts, and transaction revision. Stale heads, missing/mismatched evidence, unavailable/failed gates, incomplete receipts, and vetoes refuse before writes. The result admits host review only; it submits no order. Keep `AnalysisSnapshot` informational-only. Preserve `MEDULLA-DECISION-ENGINE-R019`, `MEDULLA-DURABLE-KERNEL-R057`, and `MEDULLA-FINANCE-PRIMITIVES-R015`.

**Exact cross-owner dependency.** Evidence does not identify the Graft SDK-accepted payload schema/version. It does not identify the complete admission-hook registry or its owner. Arbora confirmation operation and receipt fields are also undefined. Medulla may stage and validate the opaque payload. Medulla must refuse promotion until those owner contracts are reviewed. Graft SDK writeback must not cause an effect. No live-order behavior or external effect is included.

Definition only; no implementation, Arbora receipt, or Graft SDK effect is claimed.

#### `MEDULLA-FINANCE-PRIMITIVES-R002.11`

**Forecast contract.** Extend shared regime-HMM kernel R002.1.14 through typed route R002.9 with `RegimeForecastV1`. Input binds model/source identities and revisions, `as_of`, ordered horizons, `horizon_unit`, policy identity/revision/digest, and held-out dataset digest. Resolve `p0`, state order, and `T` only from the pinned model/source revision. Refuse caller overrides. Return `p_h=p0*T^h`, ordered vectors, top-state probability, calibration evidence, and bound digests. Require finite nonnegative stochastic rows/vectors within model-declared precision.

Each held-out row binds `{sample_id, model_revision, source_series_id, origin_as_of, horizon, ordered_state_ids, predicted_probability_vector, realized_state_id, target_observation_at, scoring_revision}`. Require unique samples, matching model/state order, and one row per source/origin/horizon. Require origin before target observation. Score categorical Brier separately per horizon with positive `N_h` meeting its policy floor. Bind dataset, model, state-order, and scoring revisions. Policy requires per-horizon Brier maxima, top-state probability floors, and positive sample floors. Each policy map covers every requested horizon exactly once. All thresholds must pass before sizing eligibility. Return horizon, dataset/model/state-order/scoring revisions, policy digest, sample count, score, applied thresholds, and pass state.

**Sizing.** R010 consumes the forecast digest and `RegimeSizingPolicyV1`. The policy binds `{policy_id, revision, instrument_id, target_weight_by_state, unit, fixed_point_scale, risk_cap}`. State weights and the risk cap use one R010 policy unit/scale. Compute `expected_target_weight_h=Σ_s p_h[s]×target_weight_s`, then apply the risk cap. Missing weights, cap excess, stale revisions, or failed calibration yield typed abstention with no sizing value. Never create an order. Preserve `MEDULLA-DECISION-ENGINE-R103`, `MEDULLA-FINANCE-PRIMITIVES-R002`, R010, R002.1.14, R002.6.2, and R009.1. R009.1 remains flip calibration, not state calibration.

**Exact policy dependency.** Source evidence sets no Brier thresholds, confidence floors, sample floors, state weights, or risk cap. A complete versioned policy must supply every value. No defaults exist. Missing or invalid values refuse sizing. Preserve source record SHA-256 `037f3edf5fe38f55ac24d6e0cc7bd85f1efc60cadf7c97b66b44978613a7acb6`. Definition only; no implementation or served evidence is claimed.
