//! Promotion evidence and champion qualification (MEDULLA-DECISION-ENGINE-R805.9,
//! R805.3, R654.3).
//!
//! [`held_out_evidence`] resolves a receipt into the one [`HeldOutEvidence`]
//! the promotion gate reads, whatever kind of subject the receipt belongs to.
//! [`qualified_champion`] is what a champion `Activate` must pass: the named
//! receipt is a committed `Promotion` decision record whose verdict is
//! `promote` for exactly that target and subject. [`record_rollouts`] stores a
//! rollout set under [`rollouts_key`] and answers its evidence.

use medulla_decision_outcome_types::decision::promotion::{
    DefinitionKind, GraderKind, HeldOutEvidence, ReceiptKind, RevisionStage, RevisionSubject,
    HELD_OUT_EVIDENCE_SCHEMA_VERSION, PROMOTION_UNEVALUATED,
};
use medulla_decision_outcome_types::decision::revision_list::{RevisionRow, REVISION_KEY_PREFIX};
use medulla_decision_outcome_types::policy_evolution::{
    is_policy_evolution_row, PolicyEvolutionRecord, StoredPolicyRecord,
};
use medulla_decision_stats::decision::promotion::{paired_rollout_evidence, rollout_set_digest};
use medulla_decision_types::decision::jobs::DecisionEvalReceipt;
use medulla_decision_types::decision::skill_evolution::RolloutSet;
use medulla_decision_types::decision::statistical::log::DecisionLogEntry;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, receipt_key, record_key};
use medulla_types::refusal_code::RefusalCode;
use medulla_work_types::orchestration::improvement::{AdoptedHarness, HarnessContrast};

use super::stat_executor::ExecutionContext;
use super::stat_harness_adoption::{ADOPTED_PREFIX, CONTRAST_PREFIX};
use super::stat_harness_contrast::run_scores_graded;
use super::stat_log::store_once;
use super::stat_pointer::Qualified;
use medulla_persistence_records::decision_jobs::read_pointer;

/// The premise fact prefix every `Promotion` decision seals its reading under.
pub(super) const PREMISE_PREFIX: &str = "promotion.";

/// Most decision-eval receipts a champion's score is searched in.
const MAX_RECEIPTS: usize = 4_096;

/// The decision artifact a rollout set is stored under, keyed by the digest
/// that names its evidence (MEDULLA-DECISION-ENGINE-R805.9).
pub(super) fn rollouts_key(receipt_digest: &str) -> String {
    format!("rollouts:{receipt_digest}")
}

/// Why a receipt did not resolve.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum EvidenceError {
    /// The receipt is absent, of another tenant, or carries no usable score.
    Refused(RefusalCode),
    /// The store could not be read.
    Store(String),
}

impl From<String> for EvidenceError {
    fn from(error: String) -> Self {
        Self::Store(error)
    }
}

/// What a receipt resolves against: the tenant's library, the caller's
/// visible graph (for the receipts that live in graph nodes) and the policy
/// whose knobs the evidence is computed under.
pub(super) struct EvidenceSources<'a> {
    pub(super) store: &'a AgentLibraryStore,
    pub(super) tenant: &'a str,
    pub(super) graph: Option<&'a GraphView>,
    pub(super) policy: &'a DecisionPolicy,
}

fn unevaluated() -> EvidenceError {
    EvidenceError::Refused(PROMOTION_UNEVALUATED)
}

/// The held-out evidence `receipt_digest` names for `subject`. A receipt of
/// another tenant resolves as absent. Writes nothing.
pub(super) fn held_out_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    receipt_digest: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    match subject {
        RevisionSubject::DecisionHead { .. } => decision_evidence(sources, subject, receipt_digest),
        RevisionSubject::Program { program_ref } => {
            match rollout_evidence(sources, subject, receipt_digest) {
                Err(EvidenceError::Refused(_)) => {
                    program_evidence(sources, program_ref, receipt_digest)
                }
                other => other,
            }
        }
        // A paired run on a pinned set is the receipt of a definition or of a
        // retrieval strategy (R805.10, R821): the evidence names that subject.
        // A harness bundle's receipt is its stored contrasts (R805.12).
        RevisionSubject::Definition {
            definition_kind: DefinitionKind::HarnessBundle,
            definition_ref,
        } => bundle_evidence(sources, subject, definition_ref, receipt_digest),
        RevisionSubject::Definition { .. } | RevisionSubject::RetrievalStrategy { .. } => {
            rollout_evidence(sources, subject, receipt_digest)
        }
        RevisionSubject::KnobSet { .. } | RevisionSubject::ModelPolicy { .. } => {
            policy_evidence(sources, subject, receipt_digest)
        }
        // A policy scope resolves through its recorded review (MEDULLA-ORCH-VOCAB-R012.7.3).
        RevisionSubject::GovernedSet { scope_ref } => {
            match super::policy_activation::review_evidence(
                sources.store,
                sources.tenant,
                sources.graph,
                scope_ref,
                receipt_digest,
            )? {
                Ok(Some(evidence)) => Ok(evidence),
                Ok(None) => Err(unevaluated()),
                Err(fault) => Err(EvidenceError::Refused(fault.refusal_code())),
            }
        }
        // The other subject kinds resolve through their own receipts (R805.10).
        _ => Err(unevaluated()),
    }
}

fn ppm(hits: u64, total: u64) -> i64 {
    if total == 0 {
        return 0;
    }
    (u128::from(hits) * 1_000_000 / u128::from(total)) as i64
}

fn top1_ppm(receipt: &DecisionEvalReceipt) -> Option<(i64, u64)> {
    let metrics = receipt.metrics.as_ref()?;
    (metrics.n_items > 0).then(|| (ppm(metrics.top1_hits, metrics.n_items), metrics.n_items))
}

fn decision_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    receipt_digest: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let Some(bytes) = sources
        .store
        .decision_artifact(sources.tenant, &receipt_key(receipt_digest))?
    else {
        return Err(unevaluated());
    };
    let receipt: DecisionEvalReceipt = decode_artifact(&bytes, "decision evaluation receipt")?;
    let (challenger, cases) = top1_ppm(&receipt).ok_or_else(unevaluated)?;
    // The champion is the head the subject's pointer serves; its score is the
    // top-1 accuracy of its own largest real evaluation.
    let (_, state) = read_pointer(sources.store, sources.tenant, &subject.pointer_key())?;
    let champion_ref = state.target().map(str::to_owned);
    let champion_score = match &champion_ref {
        Some(head) => champion_top1_ppm(sources, head)?,
        None => None,
    };
    Ok(HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: subject.clone(),
        challenger_ref: receipt.head_digest.clone(),
        champion_ref: champion_ref.filter(|_| champion_score.is_some()),
        receipt_kind: ReceiptKind::DecisionEval,
        receipt_digest: receipt.receipt_digest.clone(),
        evaluation_set_digest: receipt.receipt_digest.clone(),
        challenger_score_ppm: challenger,
        champion_score_ppm: champion_score,
        delta_ppm: champion_score.map_or(0, |champion| challenger - champion),
        case_count: cases,
        independent: !receipt.synthetic,
        grader_kind: GraderKind::Annotation,
        measurement: None,
        review: None,
    })
}

/// The top-1 accuracy of the real evaluation of `head` over the most items.
fn champion_top1_ppm(sources: &EvidenceSources, head: &str) -> Result<Option<i64>, String> {
    let mut best: Option<(u64, i64)> = None;
    for (_, bytes) in
        sources
            .store
            .decision_artifacts_with_prefix(sources.tenant, "receipt:", MAX_RECEIPTS)?
    {
        let receipt: DecisionEvalReceipt = decode_artifact(&bytes, "decision evaluation receipt")?;
        if receipt.head_digest != head || receipt.synthetic {
            continue;
        }
        if let Some((score, items)) = top1_ppm(&receipt) {
            if best.is_none_or(|(most, _)| items > most) {
                best = Some((items, score));
            }
        }
    }
    Ok(best.map(|(_, score)| score))
}

fn rollout_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    receipt_digest: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let Some(bytes) = sources
        .store
        .decision_artifact(sources.tenant, &rollouts_key(receipt_digest))?
    else {
        return Err(unevaluated());
    };
    let set: RolloutSet = decode_artifact(&bytes, "rollout set")?;
    let mut evidence = paired_rollout_evidence(&set, sources.policy);
    if matches!(
        subject,
        RevisionSubject::Definition { .. } | RevisionSubject::RetrievalStrategy { .. }
    ) {
        evidence.subject = subject.clone();
    } else if &evidence.subject != subject {
        return Err(unevaluated());
    }
    Ok(evidence)
}

/// The most adoption rows a harness bundle's evidence is read from.
const MAX_ADOPTIONS: usize = 100_000;

/// The domain of the digest that names a bundle's evaluation set.
const BUNDLE_SET_DOMAIN: &str = "eg/harness-bundle-evaluation-set/v1";

/// A Q32 contrast delta as millionths.
fn q32_to_micros(q32: i64) -> i64 {
    i64::try_from((i128::from(q32) * 1_000_000) >> 32).unwrap_or(i64::MAX)
}

/// The evidence of a harness bundle, `Definition { HarnessBundle,
/// <agent_ref>:<task_class> }` (MEDULLA-DECISION-ENGINE-R805.12). The receipt
/// is the id of the adoption the challenger is (an `Evaluated` revision of the
/// bundle). The evidence is read from the stored `HarnessContrast` of every
/// adoption of that agent and task class: the challenger scores the mean of
/// the baseline runs' `:Score` values plus the mean `eval_score_delta`, the
/// champion (when the pointer names one) scores the baseline mean, the case
/// count is the number of contrasts and the evaluation set is the digest of
/// the sorted conditions digests. It is independent only when every score of
/// every baseline run has a grader kind that may decide a promotion.
fn bundle_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    definition_ref: &str,
    adoption_id: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let graph = sources.graph.ok_or_else(unevaluated)?;
    let mut challenger_known = false;
    let mut contrasts: Vec<HarnessContrast> = Vec::new();
    for (_, bytes) in sources.store.decision_artifacts_with_prefix(
        sources.tenant,
        ADOPTED_PREFIX,
        MAX_ADOPTIONS,
    )? {
        let adopted: AdoptedHarness = decode_artifact(&bytes, "adopted harness")?;
        if format!("{}:{}", adopted.agent_ref, adopted.task_class) != definition_ref {
            continue;
        }
        challenger_known |= adopted.node_id() == adoption_id;
        let Some(contrast_ref) = &adopted.contrast_ref else {
            continue;
        };
        let Some(bytes) = sources
            .store
            .decision_artifact(sources.tenant, &format!("{CONTRAST_PREFIX}{contrast_ref}"))?
        else {
            continue;
        };
        let contrast: HarnessContrast = decode_artifact(&bytes, "harness contrast")?;
        if !contrasts.contains(&contrast) {
            contrasts.push(contrast);
        }
    }
    if !challenger_known || contrasts.is_empty() {
        return Err(unevaluated());
    }
    let mut baseline_runs: Vec<&str> = contrasts
        .iter()
        .map(|contrast| contrast.run_a_trace_id.as_str())
        .collect();
    baseline_runs.sort_unstable();
    baseline_runs.dedup();
    let scores: Vec<(i64, bool)> = baseline_runs
        .iter()
        .flat_map(|run| run_scores_graded(graph, run))
        .collect();
    let independent_scores: Vec<i64> = scores
        .iter()
        .filter_map(|(ppm, independent)| independent.then_some(*ppm))
        .collect();
    if independent_scores.is_empty() {
        return Err(unevaluated());
    }
    let baseline_mean =
        independent_scores.iter().sum::<i64>() / i64::try_from(independent_scores.len()).unwrap_or(1);
    let delta_mean = contrasts
        .iter()
        .map(|contrast| q32_to_micros(contrast.eval_score_delta))
        .sum::<i64>()
        / i64::try_from(contrasts.len()).unwrap_or(1);
    let challenger = (baseline_mean + delta_mean).clamp(0, 1_000_000);
    let (_, state) = read_pointer(sources.store, sources.tenant, &subject.pointer_key())?;
    let champion_ref = state.target().map(str::to_owned);
    // The baseline run of every contrast is the incumbent harness, so its
    // mean score is the champion's score whether or not a pointer is seated.
    let champion_score = Some(baseline_mean);
    let mut digests: Vec<&str> = contrasts
        .iter()
        .map(|contrast| contrast.conditions_digest.as_str())
        .collect();
    digests.sort_unstable();
    let set_digest = medulla_query_types::solve_scalar::digest_text(BUNDLE_SET_DOMAIN, &digests);
    Ok(HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: subject.clone(),
        challenger_ref: adoption_id.to_owned(),
        champion_ref,
        receipt_kind: ReceiptKind::HarnessContrast,
        receipt_digest: set_digest.clone(),
        evaluation_set_digest: set_digest,
        challenger_score_ppm: challenger,
        champion_score_ppm: champion_score,
        delta_ppm: challenger - baseline_mean,
        case_count: contrasts.len() as u64,
        independent: scores.iter().all(|(_, independent)| *independent),
        grader_kind: GraderKind::Human,
        measurement: None,
        review: None,
    })
}

fn program_evidence(
    sources: &EvidenceSources,
    program_ref: &str,
    revision_ref: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let node = sources
        .graph
        .and_then(|graph| graph.node_row_object(revision_ref))
        .ok_or_else(unevaluated)?;
    if node.get("type").and_then(serde_json::Value::as_str) != Some("ProgramRevision")
        || node.get("program_ref").and_then(serde_json::Value::as_str) != Some(program_ref)
    {
        return Err(unevaluated());
    }
    let held_out = node.get("held_out").ok_or_else(unevaluated)?;
    serde_json::from_value(held_out.clone()).map_err(|_| unevaluated())
}

fn policy_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    record_id: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let row = sources
        .graph
        .and_then(|graph| graph.node_row_object(record_id))
        .filter(|row| is_policy_evolution_row(row))
        .ok_or_else(unevaluated)?;
    let stored = StoredPolicyRecord::from_row(&row).ok_or_else(unevaluated)?;
    if stored.tenant_id != sources.tenant
        || stored.record_id != record_id
        || stored.verify_identity().is_err()
    {
        return Err(unevaluated());
    }
    let PolicyEvolutionRecord::PolicyEvaluation { record } = &stored.record else {
        return Err(unevaluated());
    };
    if &record.subject != subject {
        return Err(unevaluated());
    }
    HeldOutEvidence::try_from(record).map_err(EvidenceError::Refused)
}

/// Store a rollout set under `rollouts:<digest>` and answer its evidence
/// (MEDULLA-DECISION-ENGINE-R654.3). The set is a tenant's own record: the
/// same set replays, and no other tenant's set is ever read back.
pub(super) fn record_rollouts(
    ctx: &ExecutionContext,
    set: RolloutSet,
) -> Result<HeldOutEvidence, String> {
    check_rollout_set(ctx, &set)?;
    let digest = rollout_set_digest(&set);
    let stored = store_once(ctx, rollouts_key(&digest), "rollout set", set, |a, b| {
        a == b
    })?;
    Ok(paired_rollout_evidence(
        &stored,
        &DecisionPolicy::engine_default(),
    ))
}

/// The refusals of a rollout set, each before anything is stored
/// (MEDULLA-DECISION-ENGINE-R654.4): a rollout missing either score is
/// `ROLLOUT_UNPAIRED`; fewer complete pairs than `knob promotion.min_cases` is
/// `ROLLOUT_TOO_FEW`; a set with no `champion_ref` while no champion is active
/// for its program has nothing to compare with, `PROMOTION_NO_BASELINE`.
fn check_rollout_set(ctx: &ExecutionContext, set: &RolloutSet) -> Result<(), String> {
    use medulla_decision_outcome_types::decision::promotion::{KNOBS, MIN_CASES};
    use medulla_decision_types::decision::knobs::KnobValue;
    use medulla_decision_types::decision::skill_evolution::{ROLLOUT_TOO_FEW, ROLLOUT_UNPAIRED};
    if let Some(rollout) = set
        .rollouts
        .iter()
        .find(|rollout| rollout.challenger_score_ppm.is_none() || rollout.champion_score_ppm.is_none())
    {
        return Err(format!(
            "{}: task `{}` under seed {} lacks a challenger or a champion score",
            ROLLOUT_UNPAIRED.as_str(),
            rollout.task_id,
            rollout.seed
        ));
    }
    let policy = DecisionPolicy::engine_default();
    let pairs = set
        .rollouts
        .iter()
        .map(|rollout| (rollout.seed, rollout.task_id.as_str()))
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64;
    let min_cases = match policy.knob(&KNOBS[MIN_CASES]) {
        KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
        _ => u64::MAX,
    };
    if pairs < min_cases {
        return Err(format!(
            "{}: {pairs} pairs are fewer than the {min_cases} a promotion needs",
            ROLLOUT_TOO_FEW.as_str()
        ));
    }
    if set.champion_ref.is_none() {
        let subject = RevisionSubject::Program {
            program_ref: set.program_ref.clone(),
        };
        let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &subject.pointer_key())?;
        if state.target().is_none() {
            return Err(format!(
                "{}: the set names no champion and no champion is active for `{}`",
                medulla_decision_types::decision::codes::PROMOTION_NO_BASELINE.as_str(),
                set.program_ref
            ));
        }
    }
    Ok(())
}

/// What a champion `Activate` is qualified by: `receipt_digest` names a
/// committed `Promotion` record of this tenant whose verdict is `promote` for
/// `target` on `subject` (MEDULLA-DECISION-ENGINE-R805.3). Any other record
/// refuses `PROMOTION_RECEIPT_MISMATCH`. A harness subject also checks that
/// the target is a revision of that subject (MEDULLA-ORCH-VOCAB-R801.7):
/// otherwise `EVALUATION_RECEIPT_MISMATCH`.
pub(super) fn qualified_champion(
    store: &AgentLibraryStore,
    tenant: &str,
    subject: &RevisionSubject,
    target: &str,
    receipt_digest: &str,
) -> Result<Qualified, String> {
    harness_target_belongs(store, tenant, subject, target)?;
    let mismatch = |detail: &str| {
        format!(
            "{}: {detail}",
            medulla_decision_types::decision::codes::PROMOTION_RECEIPT_MISMATCH.as_str()
        )
    };
    let Some(bytes) = store.decision_artifact(tenant, &record_key(receipt_digest))? else {
        return Err(mismatch("no committed decision record has that id"));
    };
    let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
    let record = &entry.record;
    if record.tenant_id != tenant
        || record.question.kind
            != medulla_decision_types::decision::statistical::QuestionKind::Promotion
    {
        return Err(mismatch(
            "the record is not a promotion decision of this tenant",
        ));
    }
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    let has = |fact: String| facts.contains(&fact.as_str());
    let verdict = |name: &str| has(format!("{PREMISE_PREFIX}verdict={name}"));
    if !verdict("promote")
        || !has(format!("{PREMISE_PREFIX}subject={}", subject.pointer_key()))
        || !has(format!("{PREMISE_PREFIX}challenger={target}"))
    {
        return Err(mismatch(
            "the record is not a promote verdict for that subject and target",
        ));
    }
    // The champion pinned at decide time must still be the active one.
    let pinned = facts
        .iter()
        .find_map(|fact| fact.strip_prefix(&format!("{PREMISE_PREFIX}champion_at_decision=")))
        .unwrap_or("none");
    let (_, state) = read_pointer(store, tenant, &subject.pointer_key())?;
    if state.target().unwrap_or("none") != pinned {
        return Err(format!(
            "{}: the champion pinned by the decision is no longer active",
            medulla_decision_types::decision::codes::PROMOTION_CAS_CONFLICT.as_str()
        ));
    }
    Ok(Qualified {
        target: target.to_string(),
        receipt_digest: receipt_digest.to_string(),
    })
}

/// The most revision rows a harness bundle target is searched in.
const MAX_REVISION_ROWS: usize = 100_000;

/// The subject checks of a harness champion (MEDULLA-ORCH-VOCAB-R801.7). A
/// `HarnessComponent` target is a version node id of that component,
/// `harness-component:<component_id>:sha256:<hex>`. A `HarnessBundle` target
/// is the id of a stored revision row of that subject at stage `Evaluated`
/// or `Retired`, so only an adoption the engine wrote (or one that held the
/// pointer before) can take it. Other subjects pass.
fn harness_target_belongs(
    store: &AgentLibraryStore,
    tenant: &str,
    subject: &RevisionSubject,
    target: &str,
) -> Result<(), String> {
    let RevisionSubject::Definition {
        definition_kind,
        definition_ref,
    } = subject
    else {
        return Ok(());
    };
    let mismatch = |detail: &str| StatisticalErrorCode::EvaluationReceiptMismatch.refusal(detail);
    match definition_kind {
        DefinitionKind::HarnessComponent => {
            let prefix = format!("harness-component:{definition_ref}:sha256:");
            if target.starts_with(&prefix) {
                Ok(())
            } else {
                Err(mismatch(&format!(
                    "the target is not a version of harness component '{definition_ref}'"
                )))
            }
        }
        DefinitionKind::HarnessBundle => {
            let wanted = RevisionRow::subject_of(&subject.pointer_key());
            let held = store
                .decision_artifacts_with_prefix(tenant, REVISION_KEY_PREFIX, MAX_REVISION_ROWS)?
                .into_iter()
                .map(|(_, bytes)| decode_artifact::<RevisionRow>(&bytes, "revision row"))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|row| {
                    row.id == target
                        && Some((row.subject_kind.clone(), row.subject_ref.clone())) == wanted
                        && matches!(row.stage, RevisionStage::Evaluated | RevisionStage::Retired)
                });
            if held {
                Ok(())
            } else {
                Err(mismatch(&format!(
                    "the target is not an evaluated adoption of harness bundle '{definition_ref}'"
                )))
            }
        }
        _ => Ok(()),
    }
}

/// The receipt a committed `Promotion` record promoted on: the value of its
/// `promotion.receipt_digest` premise. `None` when the record is absent or is
/// not a promotion decision of this tenant.
pub(super) fn promoted_receipt(
    store: &AgentLibraryStore,
    tenant: &str,
    record_id: &str,
) -> Result<Option<String>, String> {
    let Some(bytes) = store.decision_artifact(tenant, &record_key(record_id))? else {
        return Ok(None);
    };
    let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
    let record = &entry.record;
    if record.tenant_id != tenant
        || record.question.kind
            != medulla_decision_types::decision::statistical::QuestionKind::Promotion
    {
        return Ok(None);
    }
    let prefix = format!("{PREMISE_PREFIX}receipt_digest=");
    Ok(record
        .premises
        .iter()
        .find_map(|premise| premise.fact.strip_prefix(&prefix).map(str::to_owned)))
}
