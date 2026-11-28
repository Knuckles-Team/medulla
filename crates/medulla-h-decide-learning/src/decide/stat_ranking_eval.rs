//! `DecisionEval` `RetrievalRankingSubmit` (MEDULLA-DECISION-ENGINE-R100.100.2):
//! compare pinned retrieval algorithm runs by hits-at-k and seal the receipt.
//!
//! The algorithm runs are produced elsewhere (`UnifiedQuery` and the retrieval
//! paths); `DecisionEval` only scores them against independently pinned
//! labels and seals what it read. The job identity, idempotency and storage
//! are those of every `DecisionEval` job (`stat_jobs::submit_job`); a run that
//! the kernel refuses never reaches a receipt.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_stats_records::decision::ranking_eval::evaluate_ranking;
use medulla_decision_types::decision::jobs::DecisionJobOutput;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use {
    medulla_decision_policy_types::decision::knob_families::ranking_eval::RankingEvalKnobs,
    medulla_decision_policy_types::decision::knob_families::ranking_eval::KNOBS,
};
use {
    medulla_decision_policy_types::decision::statistical::ranking_eval::AlgorithmScore,
    medulla_decision_policy_types::decision::statistical::ranking_eval::RetrievalRankingReceipt,
    medulla_decision_policy_types::decision::statistical::ranking_eval::RetrievalRankingRequest,
};

use super::stat_jobs::JobRun;
use medulla_h_decide_base::decide::stat_support::resolve_policy;

/// Score every run of `request` under the tenant's default decision policy
/// and seal the receipt. Refusals are the kernel's typed codes.
pub(super) fn run(store: &AgentLibraryStore, request: &RetrievalRankingRequest) -> JobRun {
    let policy = resolve_policy(store, &request.tenant_id, &DecisionPolicyRef::Default)?;
    let knobs = RankingEvalKnobs::from_policy(&policy.policy);
    let hits = evaluate_ranking(request, &knobs)?;
    let scores: Vec<AlgorithmScore> = request
        .algorithms
        .iter()
        .zip(hits)
        .map(|(run, (_, hits))| AlgorithmScore::sealed(run, hits))
        .collect();
    let knob_premises = KNOBS
        .iter()
        .map(|spec| format!("knob:{}={}", spec.name, policy.policy.knob(spec).render()))
        .collect();
    let receipt = RetrievalRankingReceipt::seal(request, scores, knob_premises);
    Ok((
        DecisionJobOutput::RankingEval {
            receipt: Box::new(receipt),
        },
        Vec::new(),
    ))
}
