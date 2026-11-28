//! The generation evaluation, split out above the generation pointer it reads.

use super::stat_retrieval::{judged_outcomes, Scope};
use medulla_decision_stats_records::decision::retrieval::Verdict;
use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_log_reader::LogReader;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::encode_artifact;
use std::collections::BTreeMap;
use {
    medulla_decision_program_types::decision::statistical::body::canonical_body_bytes,
    medulla_decision_program_types::decision::statistical::body::content_digest_of,
};
use {
    medulla_decision_types::decision::statistical::retrieval_generation::GenerationEvalRequest,
    medulla_decision_types::decision::statistical::retrieval_generation::GenerationEvaluated,
};
use {
    medulla_h_decide_base::decide::stat_generation::check_request,
    medulla_h_decide_base::decide::stat_generation::receipt_key,
    medulla_h_decide_base::decide::stat_generation::receipt_of,
    medulla_h_decide_base::decide::stat_generation::spaces,
    medulla_h_decide_base::decide::stat_generation::Generations,
    medulla_h_decide_base::decide::stat_generation::Tally,
};

/// Cited units of every independently judged successful run the caller sees.
pub(super) fn judged_citations(
    store: &AgentLibraryStore,
    reader: &LogReader,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    Ok(judged_outcomes(store, reader, Scope::ALL)?
        .into_iter()
        .filter(|judged| judged.verdict == Verdict::Success)
        .map(|judged| {
            let outcome = judged.stored.outcome;
            (outcome.record_id, outcome.cited.iter().cloned().collect())
        })
        .collect())
}

/// Dual-serve the judged runs `request` names and store the receipt under the
/// served tenant `tenant`.
pub fn evaluate_generation(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &GenerationEvalRequest,
    gens: &Generations,
    tenant: &str,
) -> Result<GenerationEvaluated, String> {
    check_request(request)?;
    let spaces = spaces(request, gens)?;
    let citations = judged_citations(ctx.store, reader)?;
    let mut tally = Tally::default();
    for item in &request.items {
        if let Some(cited) = citations.get(&item.record_id) {
            tally.add(item, cited, gens, usize::from(request.top_k));
        }
    }
    let receipt = receipt_of(request, gens, spaces, &tally);
    let receipt_digest = content_digest_of(&canonical_body_bytes(&receipt)?);
    let evaluated = GenerationEvaluated {
        receipt_digest,
        receipt,
    };
    let key = receipt_key(&evaluated.receipt_digest);
    ctx.store
        .put_decision_artifacts(tenant, &[(key, encode_artifact(&evaluated)?)])?;
    Ok(evaluated)
}
