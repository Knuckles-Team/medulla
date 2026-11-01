//! The federation implementation of the executor's foreign-source seam.

use medulla_types::wire::ForeignSourceSpec;

use super::session::FederationSession;
use super::trace::{redacted_label, EstimateProvenance};
use crate::algebra::Op;
use crate::budget::FederationBudget;
use crate::exec::foreign::{ForeignExecutor, ForeignFragmentReport};
use crate::exec::PlanCtx;
use crate::federation::ForeignSourceRegistry;
use crate::rowset::RowSet;

/// What one query binds to reach foreign sources: the owner's registry and the per-query
/// federation session (budget, LIMIT hints, fragment trace). Either may be absent.
#[derive(Clone, Copy, Default)]
pub struct FederatedExec<'a> {
    /// The owner's foreign-source registry, if any is registered.
    pub registry: Option<&'a ForeignSourceRegistry>,
    /// The per-query federation session, if one is bound.
    pub session: Option<&'a FederationSession>,
}

impl<'a> FederatedExec<'a> {
    /// An executor over `registry` and `session`.
    pub fn new(
        registry: Option<&'a ForeignSourceRegistry>,
        session: Option<&'a FederationSession>,
    ) -> Self {
        Self { registry, session }
    }

    /// An executor over a registry only (each foreign op runs under a fresh default budget).
    pub fn with_registry(registry: &'a ForeignSourceRegistry) -> Self {
        Self::new(Some(registry), None)
    }

    /// An executor over a session only.
    pub fn with_session(session: &'a FederationSession) -> Self {
        Self::new(None, Some(session))
    }
}

impl ForeignExecutor for FederatedExec<'_> {
    fn prepare(&self, _ctx: &PlanCtx, ops: &[Op]) {
        if let Some(session) = self.session {
            session.prepare(ops);
        }
    }

    fn foreign_scan(
        &self,
        op: &Op,
        input: RowSet,
        source: &ForeignSourceSpec,
        join: bool,
        _ctx: &PlanCtx,
    ) -> Result<RowSet, String> {
        super::run::foreign_scan(op, input, source, join, self.registry, self.session)
    }

    fn foreign_named(&self, op: &Op, name: &str, _ctx: &PlanCtx) -> Result<RowSet, String> {
        super::run::foreign_named(op, name, self.registry, self.session)
    }

    fn narrow_budget(&self, budget: FederationBudget) -> bool {
        match self.session {
            Some(session) => {
                session.narrow_budget(budget);
                true
            }
            None => false,
        }
    }

    fn fragment_reports(&self) -> Vec<ForeignFragmentReport> {
        let Some(session) = self.session else {
            return Vec::new();
        };
        session
            .trace()
            .iter()
            .map(|fragment| {
                let source = redacted_label(&fragment.source);
                let estimate = match fragment.estimate {
                    EstimateProvenance::NotNeeded => "not-needed".to_string(),
                    EstimateProvenance::Default => "default".to_string(),
                    EstimateProvenance::Learned { samples, rows } => {
                        format!("learned(samples={samples}, rows={rows:.2})")
                    }
                };
                ForeignFragmentReport {
                    stage: format!(
                        "REMOTE {source} strategy={:?} keys={} limit={:?} requests={} fetched={} kept={} estimate={estimate}",
                        fragment.strategy, fragment.keys_pushed, fragment.limit_pushed,
                        fragment.requests, fragment.rows_fetched, fragment.rows_kept,
                    ),
                    estimated_rows: match fragment.estimate {
                        EstimateProvenance::Learned { rows, .. } => rows,
                        _ => 0.0,
                    },
                    rows: fragment.rows_kept as u64,
                    micros: fragment.elapsed_ms.saturating_mul(1000),
                }
            })
            .collect()
    }
}
