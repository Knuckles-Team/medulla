# Research and decisions: finance primitives

## Kernel locations (R002.1.11 to R002.1.14)

Decision: the Kalman, Ornstein-Uhlenbeck, ADF and regime kernels live where they already
are: `series/kalman.rs` (the filter), `evaluation/statarb.rs` (log-variance Kalman volatility,
ADF, OU) and `evaluation/regime_hmm.rs` (Baum-Welch and Viterbi). The row text named new
files (`series/ou.rs`, `evaluation/stationarity.rs`, `series/hmm.rs`); splitting working
code across new files adds churn and no behaviour, so the L1 tests bind to the real files.
The finance crate keeps no copy: `medulla-finance-quant` re-exports the numeric kernels
for the retained finance Methods until the alias-removal rows (R002.2.2.x) retire those
Methods and the re-exports with them.

## Refusal codes

- Kalman: a filter whose q, r and initial variance (or measurement coefficient) are all
  zero has no innovation variance and refuses `KERNEL_SINGULAR`; a negative q, r or p0
  refuses `KERNEL_PARAM_INVALID`. Zero q is a valid constant-state model, so "not positive"
  in the row means "negative".
- OU: a fitted AR(1) coefficient outside (0, 1) refuses `KERNEL_SINGULAR` instead of the
  old silent fallback to theta = 1/dt.
- ADF: `max_lag` above n/4 refuses `KERNEL_PARAM_INVALID`.
- Regime HMM: more than `MAX_REFIT_OBSERVATIONS` (100000) observations refuse
  `KERNEL_STATE_BUDGET`; the work budget still applies.

## Brier caller migration (R002.2.1.10)

Decision: the wrapper `quant::brier_score` (truncate to the shorter array, zero fallback) is
deleted. The retained `Method::FinanceBrierScore` arm calls the registry's `brier` op and
maps a kernel refusal to `INVALID_ARGUMENT` with the stable code first. Removing the Method
itself is R002.2.2.10 (a wire change with generated clients), which keeps its own test.

## Golden fixtures name published data (R013.2.2, R013.2.3, R013.2.5)

Decision (owner rule FIN-R013.2.6): a golden fixture is either computed in the test by an
independent oracle that implements the published formula, or typed from a published table
with its source cited here. Each family's `manifest.json` records the source identifier,
version, as-of time, session, currency, scale and rounding; its SHA-256 digest is computed
over `vectors.json` at test time.

- `corporate_action`: oracle of the split and dividend formulas (source id
  `urn:medulla:oracle:split-and-dividend-formula`, version 1). Quantity scales by `to/from`,
  basis is preserved, dividend cash is open quantity times the per-share amount; a corrected
  split is a higher revision known later, so the `known_at` filter picks the revision.
- `dst_session`: expected labels typed from the published regular hours (09:30 to 16:00
  local, pre-market from 04:00, post-market to 20:00) and the 2026 United States daylight-saving
  transition instants (source id `urn:medulla:published:us-equity-regular-hours`, version 1).
- `futures_roll`: oracle of the roll formula (source id
  `urn:medulla:oracle:futures-roll-formula`, version 1): roll cash is quantity times multiplier
  times the settlement difference; position value is quantity times multiplier times the
  successor settlement.

## Accounting projection (R013.2.6)

Decisions: the projection supports `fifo` and `lifo`; average cost and specific lot answer
`BASIS_POLICY_UNSUPPORTED` until a policy row defines their election input. Corporate actions
are `Activity` records of kind `split` or `dividend` resolved with `corporate_actions::resolve`
(identity is listing and effective time; a higher revision known later supersedes). At one
instant a corporate action applies before a trade. A futures roll is a `transfer_out`
`Activity` that names its successor contract and carries both contracts' settlement, expiry,
tick size and multiplier; it adds roll figures and does not alter the lot ledger.

## Regime forecast: pinned model, gate and sizing (R002.11)

- Decision: the pinned model is a finance-v1 record kind `RegimeModel` written through `FinanceRecordPut`, because the served record path already owns tenant scope, shape validation, record heads and the audit trail; a second store or a new write method would duplicate it (R002.11.0). A put replaces the node, so the stored revision is the single pin and an older revision is stale.
- Decision: `forecast` lives in `crates/medulla-numeric/src/evaluation/regime_forecast.rs` beside the HMM fit in `evaluation/regime_hmm.rs`, which is where R002.1.14 placed the fit; it is the single owner of the multi-step transition powers. The wire types live in `crates/medulla-types/src/regime_forecast.rs` so `numeric_kernel.rs` stays under the file-size limit.
- Decision: the held-out samples and the calibration policy travel on the forecast request, because the handler is a stateless read of the pinned model and the gate must be reproducible from the request alone; the result carries a dataset digest and the policy digest.
- Decision: a failed threshold is a result (`passed: false`), a malformed input is a refusal, and the sizing input abstains (`SIZING_ABSTAIN`, reason first in the detail) when any horizon failed, the digest differs from the recomputed forecast, a state has no weight, or the capped figure exceeds the risk cap. No sizing value is returned on abstention.
- Decision: `finance.regime_forecast.max_brier` is the ceiling of a policy threshold (the largest possible Brier score); the threshold is never defaulted from it.

## State-space filter home (decided 2026-11-13)
The matrix recurrence, the four profile constructors, the innovation-shift detector and the robust update live on `StateSpaceFilter` (`crates/medulla-numeric/src/series/state_space.rs`, re-exported from `series/kalman.rs`). The scalar `Kalman` keeps its fields and serialised form, so `Spec::KalmanLevel` and `Spec::KalmanBeta` checkpoints stay bit-identical; one recurrence, no second state store. Each operation computes into locals and commits at the end, so every refusal leaves the filter unchanged. The detector phase is the explicit machine `innovation_shift` (Seeding, Armed, Cooldown) with one transition function. Retunes scale `R` uniformly, so the eigenvalue bounds of the model revision are bounds on the cumulative scale (1/16 to 16 by default; `Fixed` in this release). Parameter digests are FNV-1a 64 fingerprints over the bit patterns of `R`.

## State-space series storage and refusals (R002.4, R002.9)
A state-space series keeps its model, checkpoint, last observation and cursor in the series metadata as one `StateSpaceState` blob behind the magic `MSS1`; ordinary derived states keep their layout, so no stored layout changes. Source fields carry the measurement then, for every profile but volatility, the design row. A late or changed point before the cursor is refused `STATE_RETRY_CONFLICT`; a different model over a held checkpoint is `STATE_CHECKPOINT_STALE`; a source too narrow or missing is `STATE_SCHEMA_UNAVAILABLE`. An append that would consume more than the retained window refuses whole (`KERNEL_STATE_BUDGET`) and keeps the prior checkpoint; re-defining the series catches up one window per call.
