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
