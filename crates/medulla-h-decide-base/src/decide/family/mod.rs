//! The question-family registry (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! One [`DecisionFamily`] trait carries each question kind's stage hooks.
//! [`registry::for_kind`] maps every `QuestionKind` to one family through a single
//! exhaustive `match`, so a kind without a family fails to compile. Each
//! family lives in its own `family/<kind>.rs` file.

#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod assemble;
pub mod classify;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod comparative_analysis;
pub mod connector_event_triage;
pub mod connector_tool_choice;
pub mod connector_write_back;
pub mod context_budget;
pub mod enrichment_schedule;
pub mod feature_match;
pub mod harness_contrast;
pub mod ingestion_lane;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod kernel;
pub mod placement;
pub mod placement_plan;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod planning_admission;
pub mod pre_tool_risk;
pub mod research_voi;
pub mod retrieval_plan;
pub mod schema_mapping;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod skill_upgrade;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod support;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod task_context;
pub mod template_choice;
pub mod thought_search;
pub mod tool_subset;

use medulla_decision_stats_records::decision::candidate::CandidateView;

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::retrieval_plan::RetrievalProviderBinding;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::ResolutionKind,
};
use {
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};

use super::stat_executor::{Executed, ExecutionContext, Pinned};

/// The stage hooks one question family carries. Every hook has a default
/// that does nothing: a family overrides only the stages it owns.
pub trait DecisionFamily {
    /// Candidates stage: the kind's gate over declared options.
    fn validate_candidates(
        &self,
        _options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Candidates stage: the kind's gate over the declared option set, run
    /// before the generic sorted-and-unique check so the kind's own duplicate
    /// refusal is the one the caller sees.
    fn precheck_declared(&self, _options: &[DeclaredOption]) -> Result<(), String> {
        Ok(())
    }

    /// Candidates stage: whether the kind accepts `CandidateSource::RankCatalog`
    /// (MEDULLA-DECISION-ENGINE-R829.1); only `rank` does.
    fn accepts_rank_catalog(&self) -> bool {
        false
    }

    /// Constraints stage: the kind's admission over the request.
    fn constraints(&self, _request: &DecideRequest) -> Result<(), String> {
        Ok(())
    }

    /// Solve stage: the kind's specialized answer; `None` falls through to
    /// the shared matrix ladder.
    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        _request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        None
    }

    /// Solve stage for a family whose answer reads the verified caller's own
    /// decision log: it receives that caller's log reader. The default is
    /// `conclude`; one family overrides one of the two, never both.
    fn conclude_with_log(
        &self,
        ctx: &ExecutionContext,
        _reader: &super::stat_log_reader::LogReader,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        self.conclude(ctx, request, policy, graph_view)
    }

    /// Solve stage, after `conclude` or the shared ladder: the kind's
    /// refinement of the executed answer (the rank family orders and filters
    /// an advisory answer; the context-budget family partitions the sources
    /// by what the ladder's head read). `request` is `None` when a logged
    /// record is replayed, which has only its stored inputs, and `views` is
    /// then empty. The default keeps the answer unchanged.
    fn refine(
        &self,
        _request: Option<&DecideRequest>,
        _pinned: &Pinned,
        _views: &[CandidateView],
        _policy: &DecisionPolicy,
        _policy_digest: &str,
        executed: Executed,
    ) -> Result<Executed, String> {
        Ok(executed)
    }

    /// Constraints stage: the provider binding the kind commits to, resolved
    /// before any candidate is ranked (MEDULLA-DECISION-ENGINE-R029.1).
    fn bind(
        &self,
        _ctx: &ExecutionContext,
        _request: &DecideRequest,
    ) -> Result<Option<RetrievalProviderBinding>, String> {
        Ok(None)
    }

    /// Record stage: the kind's extra premises, derived from the request, from
    /// what was executed and from the digest of the policy in force.
    fn premises(
        &self,
        _request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        Vec::new()
    }

    /// Whether the kind routes a natural-language utterance to a template and
    /// binds it (`stat_nl::binding`); only `template_choice` does
    /// (MEDULLA-DECISION-ENGINE-R001.101.3).
    fn binds_utterance(&self) -> bool {
        false
    }

    /// Record stage: the resolution kind the kind seals its outcome with;
    /// `None` keeps the generic one.
    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        None
    }

    /// Record stage: the evidence class the kind seals its outcome with;
    /// `None` keeps the generic one.
    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        None
    }

    /// Conclude stage: the kind's refusal mapping.
    fn refusals(&self, detail: String) -> String {
        detail
    }
}
