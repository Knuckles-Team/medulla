//! The seam through which the executor reaches foreign (federated) sources.
//!
//! `exec` knows nothing of registries, sessions or transports: a [`PlanCtx`] carries one
//! optional [`ForeignExecutor`], and `Op::Foreign` / `Op::ForeignScan` / the pre-run
//! `prepare` hook call through it. The federation layer implements the trait.

use medulla_query_types::wire::{ForeignSourceSpec, KeptPlanReport};

use super::PlanCtx;
use crate::budget::FederationBudget;
use crate::rowset::RowSet;
use medulla_query_types::wire::Op;

/// One remote fragment of the current query, as the UQL stage report shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ForeignFragmentReport {
    /// The stage label (`REMOTE <source> strategy=… `).
    pub stage: String,
    /// The learned row estimate, `0.0` when none.
    pub estimated_rows: f64,
    /// Rows the plan kept from the fragment.
    pub rows: u64,
    /// Time spent in the fragment, in microseconds.
    pub micros: u64,
}

/// Runs the foreign leaves of a plan (federation optimizer, registry, per-query session).
pub trait ForeignExecutor: Sync {
    /// Record the LIMIT hints of the plan about to run.
    fn prepare(&self, ctx: &PlanCtx, ops: &[Op]);

    /// `Op::ForeignScan { source, join }`: a source read, or a foreign∩local join.
    fn foreign_scan(
        &self,
        op: &Op,
        input: RowSet,
        source: &ForeignSourceSpec,
        join: bool,
        ctx: &PlanCtx,
    ) -> Result<RowSet, String>;

    /// `Op::Foreign { name }`: the named source's rows replace the input.
    fn foreign_named(&self, op: &Op, name: &str, ctx: &PlanCtx) -> Result<RowSet, String>;

    /// Narrow the bound per-query budget (never widen). `false` when no per-query
    /// session is bound.
    fn narrow_budget(&self, budget: FederationBudget) -> bool;

    /// The fragments the bound per-query session recorded so far.
    fn fragment_reports(&self) -> Vec<ForeignFragmentReport>;

    /// The kept plan of each foreign source `ops` reads that has one, in plan order
    /// (MEDULLA-FEDERATED-QUERY-R091.5). None by default.
    fn kept_plans(&self, _ops: &[Op]) -> Vec<KeptPlanReport> {
        Vec::new()
    }

    /// The baseline plan digest of the first foreign source `ops` reads that has observed
    /// runs, named while no plan is kept (MEDULLA-CQ-ORCH-R011.3). None by default.
    fn baseline_digest(&self, _ops: &[Op]) -> Option<String> {
        None
    }
}
