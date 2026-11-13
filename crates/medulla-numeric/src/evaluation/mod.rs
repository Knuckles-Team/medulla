//! Evaluation kernels (EH-530): the generic forms of what the finance module computed
//! in private — the normal special functions, backtest validation (purged CPCV,
//! Deflated Sharpe, PBO, Diebold-Mariano) and predictive skill (IC, IR, effective
//! breadth, the FeatureSkill report). The finance Methods call these; so do the
//! decision layer and UQL.

pub mod backtest;
pub mod regime_forecast;
pub mod skill;
pub mod special;

// MEDULLA-FINANCE-PRIMITIVES-R002.1.11..R002.1.14 and R002.2.5: the stat-arb and regime
// kernels and the registry that serves every evaluation kernel by op.
#[cfg(feature = "evaluate")]
pub mod regime_hmm;
#[cfg(feature = "evaluate")]
pub mod registry;
#[cfg(feature = "evaluate")]
pub mod statarb;
