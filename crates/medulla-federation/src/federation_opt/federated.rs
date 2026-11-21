//! The federation implementation of the executor's foreign-source seam.

use medulla_query_types::wire::{ForeignSourceSpec, KeptPlanReport};

use super::session::FederationSession;
use super::trace::{redacted_label, EstimateProvenance};
use crate::federation::ForeignSourceRegistry;
use medulla_exec::algebra::Op;
use medulla_exec::budget::FederationBudget;
use medulla_exec::exec::foreign::{ForeignExecutor, ForeignFragmentReport};
use medulla_exec::exec::PlanCtx;
use medulla_exec::rowset::RowSet;

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

    fn kept_plans(&self, ops: &[Op]) -> Vec<KeptPlanReport> {
        let Some(scope) = self.session.and_then(FederationSession::cache_scope) else {
            return Vec::new();
        };
        if !super::enabled() {
            return Vec::new();
        }
        ops.iter()
            .filter_map(|op| {
                // Resolve exactly as `foreign_scan` and `foreign_named` do, so the shape
                // digest is the key `kept_for` reads at run time.
                let remote = match op {
                    Op::ForeignScan { source, .. } => super::remote::resolve(source, self.registry),
                    Op::Foreign { name } => self
                        .registry
                        .and_then(|registry| super::remote::resolve_named(name, registry)),
                    _ => None,
                }?;
                let label = remote.label();
                let shape = super::plan_evolution::shape_digest_of(&label);
                let Some(kept) = super::plan_evolution::kept_plan(scope.owner_salt(), &shape)
                else {
                    // No plan is kept: the report names the baseline plan in force and no
                    // receipt (MEDULLA-CQ-ORCH-R011.3). A shape never observed has none.
                    let baseline =
                        super::plan_evolution::baseline_plan_of(scope.owner_salt(), &shape)?;
                    return Some(KeptPlanReport {
                        source: redacted_label(&label),
                        plan_digest: baseline.clone(),
                        receipt_job_id: String::new(),
                        baseline_digest: baseline,
                        replays: 0,
                        p50_before_us: 0,
                        p50_after_us: 0,
                        bytes_before: 0,
                        bytes_after: 0,
                    });
                };
                Some(KeptPlanReport {
                    source: redacted_label(&label),
                    plan_digest: kept.plan_digest,
                    receipt_job_id: kept.receipt_job_id,
                    baseline_digest: kept.evidence.baseline_digest,
                    replays: kept.evidence.replays,
                    p50_before_us: kept.evidence.p50_before_us,
                    p50_after_us: kept.evidence.p50_after_us,
                    bytes_before: kept.evidence.bytes_before,
                    bytes_after: kept.evidence.bytes_after,
                })
            })
            .collect()
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
                        "REMOTE {source} strategy={:?} keys={} limit={:?} requests={} fetched={} kept={} estimate={estimate} mode={}{}",
                        fragment.strategy, fragment.keys_pushed, fragment.limit_pushed,
                        fragment.requests, fragment.rows_fetched, fragment.rows_kept,
                        super::trace::mode_word(fragment.mode),
                        // MEDULLA-FEDERATED-QUERY-R091.3: the digest of a kept plan that
                        // changed this fragment's choice.
                        fragment
                            .kept_plan
                            .as_deref()
                            .map_or_else(String::new, |digest| format!(" kept_plan={digest}")),
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
