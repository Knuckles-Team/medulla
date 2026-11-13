//! `DecisionLog`, served (EH-060, EH-061, EH-012), and the logged-dataset
//! builder `DecisionFit`/`DecisionEval` read labels from.
//!
//! Every read filters by each record's own visibility before a record, an
//! evaluation, a count or a rate is formed: tenant-wide for library-sourced
//! records, the committing principal only for graph-sourced ones.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::collections::BTreeMap;
use std::time::Instant;

use tracing::Instrument;

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::features::outcome_rate_key;
use medulla_decision_stats::decision::transfer::{
    robust_transfer, skill_candidates, SkillKnobs, SkillRow, TransferKnobs, TransferRow,
};
use medulla_decision_stats::{
    decision::aggregate::aggregate, decision::aggregate::AggregateRules,
    decision::aggregate::JoinedRecord,
};
use medulla_decision_types::decision::transfer::{is_digest, TransferEstimate, TransferRequest};
use medulla_decision_types::{
    decision::statistical::dataset::ItemLabel, decision::statistical::dataset::LabelledDataset,
    decision::statistical::dataset::LabelledItem, decision::statistical::dataset::LoggedOutcome,
    decision::statistical::dataset::OutcomeEvaluation,
    decision::statistical::dataset::PropensitySource,
    decision::statistical::dataset::LABELLED_DATASET_SCHEMA_VERSION,
};
use medulla_decision_types::{
    decision::statistical::features::FeatureKind,
    decision::statistical::features::FeatureSchemaBody,
};
use medulla_decision_types::{
    decision::statistical::log::DecisionLogCommitted, decision::statistical::log::DecisionLogEntry,
    decision::statistical::log::DecisionLogOp,
    decision::statistical::log::DecisionOutcomeEvaluation, decision::statistical::log::EntryInputs,
    decision::statistical::log::OutcomeAggregate,
    decision::statistical::log::OutcomeAggregateRequest,
    decision::statistical::log::RecordVisibility, decision::statistical::log::StoredEvaluation,
    decision::statistical::log::DECISION_LOG_SCHEMA_VERSION,
};
use medulla_decision_types::{
    decision::statistical::FeatureMatrixRef, decision::statistical::StatisticalDecisionRecord,
    decision::statistical::StatisticalErrorCode, decision::statistical::StatisticalOutcome,
};
use medulla_decision_types::{
    decision::CandidateSourceRecord, decision::QuantScaleTag, decision::RecordWindow,
};
use medulla_types::contract::BoundedVec;

use super::stat_executor::ExecutionContext;
use super::stat_replay::{replay_view, replay_with_view, verify_graph_rows, GraphSnapshots};
use super::stat_resolve::resolve;
use super::stat_retention::{compact, full_record, verify, Retention};
use super::stat_slate::{evaluated_slates, slate_of, SLATE_QUESTION};
use super::stat_support::{default_statistical_policy, pinned_entry, refusal};
use super::telemetry;
use super::SharedState;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::evaluation_key;
use medulla_persistence_records::decision_jobs::record_key;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;

/// Most log rows one read walks before refusing as unbounded.
pub(super) const MAX_LOG_ROWS: usize = 100_000;

/// Who is reading or writing the log.
pub(super) struct LogReader {
    pub(super) tenant_id: String,
    pub(super) principal: String,
    /// The verified request context's role names: a role-named evaluator's
    /// grant admits a holder (EH-395). Empty where no evaluation is served.
    pub(super) roles: Vec<String>,
    /// How compacted inputs are restored when a read needs them.
    pub(super) retention: Retention,
}

impl LogReader {
    /// A reader that needs no compacted inputs (aggregates, visibility).
    pub(super) fn of(verified: &VerifiedRequestContext) -> Self {
        Self {
            tenant_id: verified.tenant().to_string(),
            principal: verified.principal_persistence_id(),
            roles: verified.roles().to_vec(),
            retention: Retention::none(),
        }
    }

    /// A reader that restores compacted inputs from the served Blob CAS.
    pub(super) async fn served(state: &SharedState, verified: &VerifiedRequestContext) -> Self {
        let retention = Retention::of(&*state.read().await, verified);
        Self {
            retention,
            ..Self::of(verified)
        }
    }

    fn sees(&self, entry: &DecisionLogEntry) -> bool {
        match &entry.visibility {
            RecordVisibility::Tenant => true,
            RecordVisibility::Principal { principal } => *principal == self.principal,
        }
    }
}

pub(super) fn executed_option(outcome: &StatisticalOutcome) -> Option<&str> {
    match outcome {
        StatisticalOutcome::Acted { option_id, .. }
        | StatisticalOutcome::Explored { option_id, .. } => Some(option_id),
        StatisticalOutcome::Advisory { .. } | StatisticalOutcome::Abstained { .. } => None,
    }
}

pub(super) fn visible_entries(
    store: &AgentLibraryStore,
    reader: &LogReader,
) -> Result<Vec<DecisionLogEntry>, String> {
    store
        .decision_artifacts_with_prefix(&reader.tenant_id, "record:", MAX_LOG_ROWS)?
        .into_iter()
        .map(|(_, bytes)| decode_artifact::<DecisionLogEntry>(&bytes, "decision log entry"))
        .filter(|entry| !matches!(entry, Ok(e) if !reader.sees(e)))
        .collect()
}

/// One earlier decision that shares the current decision's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MatchedTrajectory {
    pub(super) record_id: String,
    /// The option that record executed.
    pub(super) option_id: String,
}

/// The digest two decisions share exactly when they ask the same question
/// with the same typed parameters (MEDULLA-DECISION-ENGINE-R020.1.1).
pub(super) fn context_digest_of(
    question_id: &str,
    params: &[medulla_decision_types::decision::statistical::TypedParam],
) -> String {
    // Outcome pins are evidence about the context, never part of it
    // (MEDULLA-DECISION-ENGINE-R134.9): a pinned and an unpinned ask match.
    let mut sorted: Vec<medulla_decision_types::decision::statistical::TypedParam> = params
        .iter()
        .map(|param| {
            let mut param = param.clone();
            if let medulla_decision_types::decision::statistical::TypedValue::SkillPlanQuery(
                query,
            ) = &mut param.value
            {
                query.outcome_pins = Default::default();
            }
            param
        })
        .collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    medulla_query_types::solve_scalar::digest_text("eg/decision-context", &(question_id, sorted))
}

/// How far back, and how many, matched records may be.
#[derive(Debug, Clone, Copy)]
pub(super) struct MatchWindow {
    pub(super) now_ms: u64,
    pub(super) window_ms: u64,
    pub(super) max_matched: usize,
}

/// Executed, committed records of the reader's own tenant AND principal whose
/// sealed `context_digest` equals `context_digest`, inside `window`, newest
/// first up to `max_matched` and returned in record id order. Another
/// principal's or tenant's entries are never read.
pub(super) fn matched_trajectories(
    store: &AgentLibraryStore,
    reader: &LogReader,
    context_digest: &str,
    window: MatchWindow,
) -> Result<Vec<MatchedTrajectory>, String> {
    let earliest = window.now_ms.saturating_sub(window.window_ms);
    let mut matched: Vec<(u64, MatchedTrajectory)> = visible_entries(store, reader)?
        .into_iter()
        .filter(|entry| {
            let record = &entry.record;
            record.caller_principal == reader.principal
                && record.inputs.context_digest.as_deref() == Some(context_digest)
                && (earliest..=window.now_ms).contains(&record.created_at_ms)
        })
        .filter_map(|entry| {
            let option_id = executed_option(&entry.record.outcome)?.to_string();
            Some((
                entry.record.created_at_ms,
                MatchedTrajectory {
                    record_id: entry.record.record_id.clone(),
                    option_id,
                },
            ))
        })
        .collect();
    matched.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.record_id.cmp(&b.1.record_id))
    });
    matched.truncate(window.max_matched);
    let mut out: Vec<MatchedTrajectory> = matched.into_iter().map(|(_, m)| m).collect();
    out.sort_by(|a, b| a.record_id.cmp(&b.record_id));
    Ok(out)
}

/// The latest committed disclosure event of `set_id` the reader may see: the
/// event answer of the newest `scan_limit` visible `InstructionDisclosure`
/// records of the reader's tenant, highest version winning
/// (MEDULLA-DECISION-ENGINE-R143.3). Another tenant's or another principal's
/// private events are never read.
pub(super) fn latest_disclosure(
    store: &AgentLibraryStore,
    reader: &LogReader,
    set_id: &str,
    scan_limit: usize,
) -> Result<Option<medulla_decision_types::decision::instruction_disclosure::DisclosureEvent>, String>
{
    use medulla_decision_types::decision::statistical::QuestionKind;
    let mut entries: Vec<DecisionLogEntry> = visible_entries(store, reader)?
        .into_iter()
        .filter(|entry| entry.record.question.kind == QuestionKind::InstructionDisclosure)
        .collect();
    entries.sort_by(|a, b| {
        b.record
            .created_at_ms
            .cmp(&a.record.created_at_ms)
            .then_with(|| a.record.record_id.cmp(&b.record.record_id))
    });
    entries.truncate(scan_limit);
    Ok(entries
        .iter()
        .filter_map(|entry| entry.record.answer.as_ref()?.disclosure_event())
        .filter(|event| event.set_id == set_id)
        .max_by_key(|event| event.version)
        .cloned())
}

pub(super) fn visible_entry(
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
) -> Result<Option<DecisionLogEntry>, String> {
    let Some(bytes) = store.decision_artifact(&reader.tenant_id, &record_key(record_id))? else {
        return Ok(None);
    };
    let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
    Ok(reader.sees(&entry).then_some(entry))
}

pub(super) fn evaluations_of(
    store: &AgentLibraryStore,
    tenant_id: &str,
    record_id: &str,
) -> Result<Vec<StoredEvaluation>, String> {
    store
        .decision_artifacts_with_prefix(
            tenant_id,
            &format!("evaluation:{record_id}:"),
            MAX_LOG_ROWS,
        )?
        .into_iter()
        .map(|(_, bytes)| decode_artifact(&bytes, "decision outcome evaluation"))
        .collect()
}

fn visibility_of(record: &StatisticalDecisionRecord, principal: &str) -> RecordVisibility {
    match record.candidate_source {
        CandidateSourceRecord::AgentLibrary { .. } => RecordVisibility::Tenant,
        CandidateSourceRecord::Graph { .. } | CandidateSourceRecord::Declared { .. } => {
            RecordVisibility::Principal {
                principal: principal.to_string(),
            }
        }
    }
}

fn commit(
    ctx: &ExecutionContext,
    reader: &LogReader,
    record: &StatisticalDecisionRecord,
    evaluator: Option<&medulla_decision_types::decision::statistical::log::NamedEvaluator>,
    replay_graph: Option<&medulla_graph_core::graph::GraphView>,
    graphs: &GraphSnapshots,
) -> Result<DecisionLogCommitted, String> {
    if record.caller_principal != reader.principal {
        return Err(
            "ACCESS_DENIED: a decision is logged by the principal that made it".to_string(),
        );
    }
    let logged = match super::stat_replay::verify_disclosure_event(ctx, reader, record)? {
        Some(verified) => verified,
        None => replay_with_view(ctx, record, replay_graph)?,
    };
    let key = record_key(&logged.record_id);
    let committed = |replayed| DecisionLogCommitted {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        record_id: logged.record_id.clone(),
        record_digest: logged.record_digest.clone(),
        replayed,
    };
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let existing: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
        if existing.record.record_digest == logged.record_digest {
            return Ok(committed(true));
        }
        return Err(refusal(
            StatisticalErrorCode::IdempotencyConflict,
            "the record id is logged with other content",
        ));
    }
    // A new record's graph rows are re-read now; an already logged record
    // answers above without it, so a repeat commit stays idempotent after the
    // graph moves on.
    verify_graph_rows(record, graphs)?;
    let entry = DecisionLogEntry {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        visibility: visibility_of(&logged, &reader.principal),
        record: Box::new(logged.clone()),
        committed_by: reader.principal.clone(),
        committed_at_ms: ctx.now_ms,
        inputs: EntryInputs::Inline,
    };
    // EH-395: a named evaluator's grant is written in the same transaction.
    let mut rows =
        super::stat_evaluator::grant_rows(ctx, &logged.record_id, &reader.principal, evaluator)?;
    rows.push((key, encode_artifact(&entry)?));
    ctx.store.put_decision_artifacts(ctx.tenant_id, &rows)?;
    Ok(committed(false))
}

/// An evaluation joins an executed statistical record, or a committed solved
/// assembly (its slate, EH-012); anything else has nothing to evaluate.
fn check_evaluable(
    ctx: &ExecutionContext,
    reader: &LogReader,
    record_id: &str,
) -> Result<(), String> {
    let invalid = |detail: &str| refusal(StatisticalErrorCode::ParameterInvalid, detail);
    match super::stat_evaluator::evaluable_entry(ctx, reader, record_id)? {
        Some(entry) if executed_option(&entry.record.outcome).is_none() => {
            Err(invalid("the record executed no option"))
        }
        Some(_) => Ok(()),
        None => match slate_of(ctx.store, ctx.tenant_id, record_id)? {
            Some(_) => Ok(()),
            None => Err(invalid("no committed record with that id is visible")),
        },
    }
}

fn evaluate(
    ctx: &ExecutionContext,
    reader: &LogReader,
    evaluation: DecisionOutcomeEvaluation,
) -> Result<StoredEvaluation, String> {
    if let Some(binding) = &evaluation.backbone {
        binding
            .validate()
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    }
    check_evaluable(ctx, reader, &evaluation.record_id)?;
    let key = evaluation_key(&evaluation.record_id, &evaluation.evaluation_id);
    let stored = StoredEvaluation {
        evaluation,
        producer: reader.principal.clone(),
        recorded_at_ms: ctx.now_ms,
    };
    store_once(ctx, key, "evaluation", stored, |existing, fresh| {
        existing.evaluation == fresh.evaluation && existing.producer == fresh.producer
    })
}

/// Store `fresh` under `key` exactly once. A re-send with the same content
/// from the same producer answers the stored row (idempotent replay);
/// anything else under that key is a conflict, never an overwrite.
pub(super) fn store_once<T: serde::Serialize + serde::de::DeserializeOwned>(
    ctx: &ExecutionContext,
    key: String,
    noun: &str,
    fresh: T,
    same: impl Fn(&T, &T) -> bool,
) -> Result<T, String> {
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let existing: T = decode_artifact(&bytes, noun)?;
        if same(&existing, &fresh) {
            return Ok(existing);
        }
        return Err(refusal(
            StatisticalErrorCode::IdempotencyConflict,
            format!("the {noun} id is recorded with other content"),
        ));
    }
    ctx.store
        .put_decision_artifacts(ctx.tenant_id, &[(key, encode_artifact(&fresh)?)])?;
    Ok(fresh)
}

/// Visible, executed records of `question_id` (or of every question) inside
/// the window, each with its evaluations.
pub(super) fn joined(
    store: &AgentLibraryStore,
    reader: &LogReader,
    question_id: Option<&str>,
    window: Option<RecordWindow>,
) -> Result<Vec<(DecisionLogEntry, Vec<StoredEvaluation>)>, String> {
    let mut out = Vec::new();
    for entry in visible_entries(store, reader)? {
        let record = &entry.record;
        let in_question = question_id.is_none_or(|q| q == record.question.question_id);
        let in_window =
            window.is_none_or(|w| (w.from_ms..=w.to_ms).contains(&record.created_at_ms));
        if !in_question || !in_window || executed_option(&record.outcome).is_none() {
            continue;
        }
        let evaluations = evaluations_of(store, &reader.tenant_id, &record.record_id)?;
        out.push((entry, evaluations));
    }
    Ok(out)
}

/// The real-world outcome aggregate the caller may read.
/// Synthetic decisions stay available in the log and labelled job datasets,
/// but cannot contribute counts, support, or pooled rates here.
pub(super) fn aggregate_log(
    store: &AgentLibraryStore,
    reader: &LogReader,
    request: &OutcomeAggregateRequest,
) -> Result<OutcomeAggregate, String> {
    let rows = joined(
        store,
        reader,
        request.question_id.as_deref(),
        Some(request.window),
    )?;
    let records: Vec<JoinedRecord> = rows
        .iter()
        .filter(|(entry, _)| !entry.record.synthetic_evidence)
        .filter_map(|(entry, evaluations)| {
            Some(JoinedRecord {
                option_id: executed_option(&entry.record.outcome)?,
                question_id: &entry.record.question.question_id,
                policy_digest: &entry.record.inputs.policy_digest,
                decider: &entry.record.caller_principal,
                evaluations,
            })
        })
        .collect();
    let slates = evaluated_slates(
        store,
        &reader.tenant_id,
        request.question_id.as_deref(),
        (request.window.from_ms, request.window.to_ms),
        MAX_LOG_ROWS,
    )?;
    let records: Vec<JoinedRecord> = records
        .into_iter()
        .chain(slates.iter().map(|(slate, evaluations)| JoinedRecord {
            option_id: &slate.option_id,
            question_id: SLATE_QUESTION,
            policy_digest: &slate.policy_digest,
            decider: &slate.decider,
            evaluations,
        }))
        .collect();
    let policy = default_statistical_policy();
    let rules = AggregateRules {
        min_support: policy.min_support,
        fidelity_floor: policy.min_outcome_fidelity,
        cross_question: request.question_id.is_none(),
    };
    let transfer = request
        .transfer
        .as_ref()
        .map(|asked| transfer_estimate(&rows, asked, policy.min_outcome_fidelity))
        .transpose()?;
    let attribution = request
        .attribution
        .as_ref()
        .map(|split| {
            super::stat_slate_attribution::attribute_slate(store, reader, split, request.window)
        })
        .transpose()?;
    let skills = skill_candidate_rows(&rows, policy.min_outcome_fidelity)?;
    Ok(OutcomeAggregate {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        min_support: policy.min_support,
        rows: aggregate(&records, &rules).map_err(|r| r.render())?,
        attribution,
        transfer,
        skill_candidates: skills,
    })
}

/// The latest `n` outcomes of `option_id` that count as labels (independent,
/// observed, uncensored, at the fidelity floor), newest first; `true` is a
/// success. Synthetic records never count. This is the skip rule's input
/// (MEDULLA-DECISION-ENGINE-R159.3): it reads the same label rule as
/// [`aggregate_log`].
pub(super) fn recent_evaluations(
    store: &AgentLibraryStore,
    reader: &LogReader,
    option_id: &str,
    n: usize,
) -> Result<Vec<bool>, String> {
    Ok(recent_labels(store, reader, option_id, n)?
        .into_iter()
        .map(|label| label.success)
        .collect())
}

/// One outcome that counts as a label: when it was recorded, which evaluation
/// recorded it and whether it succeeded.
pub(super) struct OutcomeLabel {
    /// The option the reviewed decision executed.
    pub(super) option_id: String,
    pub(super) recorded_at_ms: u64,
    pub(super) evaluation_id: String,
    pub(super) success: bool,
}

/// The latest `n` labels of `option_id`, newest first, under the same label
/// rule as [`recent_evaluations`] (MEDULLA-DECISION-ENGINE-R159.5).
pub(super) fn recent_labels(
    store: &AgentLibraryStore,
    reader: &LogReader,
    option_id: &str,
    n: usize,
) -> Result<Vec<OutcomeLabel>, String> {
    recent_labels_where(store, reader, |executed| executed == option_id, n)
}

/// The latest `n` labels of every option `wanted` accepts, newest first, under
/// the one label rule (MEDULLA-DECISION-ENGINE-R706.3).
pub(super) fn recent_labels_where(
    store: &AgentLibraryStore,
    reader: &LogReader,
    wanted: impl Fn(&str) -> bool,
    n: usize,
) -> Result<Vec<OutcomeLabel>, String> {
    Ok(recent_label_set_where(store, reader, wanted, n)?.labels)
}

/// The labels of [`recent_labels_where`] with the count of evaluations that
/// did not count: claims, self-reports and traces below the fidelity floor
/// (`LabelUse::Refused`).
pub(super) struct LabelSet {
    /// The latest labels, newest first.
    pub(super) labels: Vec<OutcomeLabel>,
    /// Evaluations of the accepted options that are refused as labels.
    pub(super) refused: usize,
}

/// The labels of [`recent_evaluations`] with the refused count of one option.
pub(super) struct RecentLabels {
    /// The latest labels, newest first; `true` is a success.
    pub(super) labels: Vec<bool>,
    /// Evaluations of the option that are refused as labels.
    pub(super) refused: usize,
}

/// The latest `n` labels of `option_id` and how many of its evaluations were
/// refused as labels.
pub(super) fn recent_labels_counted(
    store: &AgentLibraryStore,
    reader: &LogReader,
    option_id: &str,
    n: usize,
) -> Result<RecentLabels, String> {
    let set = recent_label_set_where(store, reader, |executed| executed == option_id, n)?;
    Ok(RecentLabels {
        labels: set.labels.into_iter().map(|label| label.success).collect(),
        refused: set.refused,
    })
}

/// The one label read: the latest `n` labels of every option `wanted` accepts
/// and the count of refused evaluations (MEDULLA-DECISION-ENGINE-R706.3).
fn recent_label_set_where(
    store: &AgentLibraryStore,
    reader: &LogReader,
    wanted: impl Fn(&str) -> bool,
    n: usize,
) -> Result<LabelSet, String> {
    use medulla_decision_stats::decision::aggregate::{label_use, LabelUse};
    let floor = default_statistical_policy().min_outcome_fidelity;
    let mut labels: Vec<OutcomeLabel> = Vec::new();
    let mut refused = 0usize;
    for (entry, evaluations) in joined(store, reader, None, None)? {
        let record = &entry.record;
        let Some(option_id) = executed_option(&record.outcome).filter(|id| wanted(id)) else {
            continue;
        };
        if record.synthetic_evidence {
            continue;
        }
        for stored in &evaluations {
            match label_use(stored, &record.caller_principal, floor) {
                LabelUse::Label(success) => labels.push(OutcomeLabel {
                    option_id: option_id.to_string(),
                    recorded_at_ms: stored.recorded_at_ms,
                    evaluation_id: stored.evaluation.evaluation_id.clone(),
                    success,
                }),
                LabelUse::Refused => refused += 1,
                LabelUse::Censored => {}
            }
        }
    }
    labels.sort_by(|a, b| {
        (b.recorded_at_ms, &b.evaluation_id).cmp(&(a.recorded_at_ms, &a.evaluation_id))
    });
    labels.truncate(n);
    Ok(LabelSet { labels, refused })
}

/// Options of the caller's visible, non-synthetic log that earned a skill
/// (MEDULLA-DECISION-ENGINE-R059.2). Writes nothing; the knobs are the
/// resolved registry values.
fn skill_candidate_rows(
    rows: &[(DecisionLogEntry, Vec<StoredEvaluation>)],
    fidelity_floor: medulla_decision_types::decision::TraceFidelityLevel,
) -> Result<BoundedVec<medulla_decision_types::decision::transfer::SkillCandidateRow, 1024>, String>
{
    let resolved = medulla_server_api::knob_config::knobs();
    let knobs = SkillKnobs {
        after_successes: resolved.transfer_skill_after_successes,
        min_rollouts: resolved.transfer_min_rollouts,
        fidelity_floor,
    };
    let evidence: Vec<SkillRow> = rows
        .iter()
        .filter(|(entry, _)| !entry.record.synthetic_evidence)
        .filter_map(|(entry, evaluations)| {
            let option_id = executed_option(&entry.record.outcome)?;
            Some(evaluations.iter().map(move |stored| SkillRow {
                question_id: &entry.record.question.question_id,
                option_id,
                decider: &entry.record.caller_principal,
                stored,
            }))
        })
        .flatten()
        .collect();
    BoundedVec::new(skill_candidates(&evidence, &knobs))
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))
}

/// The robust transfer estimate of one skill over the caller's visible,
/// non-synthetic log (MEDULLA-DECISION-ENGINE-R059.1.3). Writes nothing. The
/// knobs are the resolved registry values; the refusals are the registered
/// `transfer::REFUSALS` codes.
fn transfer_estimate(
    rows: &[(DecisionLogEntry, Vec<StoredEvaluation>)],
    request: &TransferRequest,
    fidelity_floor: medulla_decision_types::decision::TraceFidelityLevel,
) -> Result<TransferEstimate, String> {
    use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
    if !is_digest(&request.skill_key) {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "transfer skill_key must be 64 lowercase hex characters",
        ));
    }
    let resolved = medulla_server_api::knob_config::knobs();
    let knobs = TransferKnobs {
        min_backbones: resolved.transfer_min_backbones,
        min_samples: resolved.transfer_min_samples,
        fidelity_floor,
    };
    let evidence: Vec<TransferRow> = rows
        .iter()
        .filter(|(entry, _)| !entry.record.synthetic_evidence)
        .flat_map(|(entry, evaluations)| {
            evaluations.iter().map(move |stored| TransferRow {
                decider: &entry.record.caller_principal,
                stored,
            })
        })
        .collect();
    let mut estimate = robust_transfer(&evidence, request, &knobs).map_err(|r| r.render())?;
    let class = match estimate.evidence_class {
        medulla_decision_types::decision::EvidenceClass::Proof => PremiseClass::Proof,
        _ => PremiseClass::Observation,
    };
    let evidence_key = format!("transfer:{}", request.skill_key);
    let observed = |fact: String| PremiseRef {
        subject: request.skill_key.clone(),
        fact,
        class,
        provenance: PremiseProvenance::Observation {
            evaluation_id: evidence_key.clone(),
        },
    };
    let knob_facts = [
        format!("knob:transfer.min_backbones={}", knobs.min_backbones),
        format!("knob:transfer.min_samples={}", knobs.min_samples),
    ];
    let knob_digest = medulla_query_types::solve_scalar::digest_text(
        "eg/transfer-knobs/v1",
        &knob_facts.join(";"),
    );
    let mut premises = vec![
        observed(format!(
            "transfer.robust_rate={}",
            estimate.robust_rate.value
        )),
        observed(format!(
            "transfer.min_backbone={}",
            estimate.min_backbone_id
        )),
    ];
    premises.extend(knob_facts.into_iter().map(|fact| PremiseRef {
        subject: "policy".to_string(),
        fact,
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Policy {
            policy_digest: knob_digest.clone(),
        },
    }));
    estimate.premises = BoundedVec::new(premises)
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    Ok(estimate)
}

fn logged_item(
    entry: &DecisionLogEntry,
    record: &StatisticalDecisionRecord,
    stored: &StoredEvaluation,
) -> Option<LabelledItem> {
    let FeatureMatrixRef::Inline {
        candidate_ids,
        values,
        ..
    } = &record.inputs.feature_matrix
    else {
        return None;
    };
    let aligned = !values.is_empty() && record.logging_propensities.len() == candidate_ids.len();
    let executed = executed_option(&record.outcome).filter(|_| aligned)?;
    let e = &stored.evaluation;
    Some(LabelledItem {
        item_id: record.record_id.clone(),
        recorded_at_ms: record.created_at_ms,
        class_key: record.question.question_id.clone(),
        candidate_ids: candidate_ids.clone(),
        features: values.clone(),
        label: ItemLabel::Logged(Box::new(LoggedOutcome {
            executed: executed.to_string(),
            logging_propensities: record.logging_propensities.clone(),
            propensity_source: PropensitySource::ExecutedPolicy,
            pinned: false,
            commit_principal: entry.committed_by.clone(),
            evaluation: OutcomeEvaluation {
                evaluation_id: e.evaluation_id.clone(),
                class: e.class,
                producer: stored.producer.clone(),
                selected_agent: e.selected_agent.clone(),
                lease_holder: e.lease_holder.clone(),
                fidelity: e.fidelity,
                success: e.success,
            },
        })),
        audit_inclusion: record
            .audit
            .filter(|a| a.sampled)
            .map(|a| a.inclusion_probability),
    })
}

/// The bandit-label dataset of one question, read from the decision log:
/// every visible, executed record computed under the feature schema whose
/// content digest is `schema_digest`, joined with its first evaluation.
pub(super) fn logged_dataset(
    store: &AgentLibraryStore,
    reader: &LogReader,
    question_id: &str,
    schema_digest: &str,
) -> Result<LabelledDataset, String> {
    let mut items = Vec::new();
    let mut names = Vec::new();
    let mut synthetic = false;
    for (entry, evaluations) in joined(store, reader, Some(question_id), None)? {
        let pin = &entry.record.inputs.feature_schema;
        let schema = pinned_entry(
            store,
            &reader.tenant_id,
            pin,
            AgentComponentKind::FeatureSchema,
        )?;
        let (Some(stored), true) = (evaluations.first(), schema.content_digest == schema_digest)
        else {
            continue;
        };
        let Some(record) = full_record(&reader.retention, &reader.tenant_id, &entry)? else {
            continue;
        };
        if let FeatureMatrixRef::Inline { feature_names, .. } = &record.inputs.feature_matrix {
            names = feature_names.iter().cloned().collect();
        }
        synthetic |= record.synthetic_evidence;
        items.extend(logged_item(&entry, &record, stored));
    }
    if items.is_empty() {
        return Err(refusal(
            StatisticalErrorCode::NoAdmissibleLabels,
            format!("no logged, evaluated decision of {question_id}"),
        ));
    }
    let invalid = |detail: String| refusal(StatisticalErrorCode::DatasetInvalid, detail);
    Ok(LabelledDataset {
        schema_version: LABELLED_DATASET_SCHEMA_VERSION,
        feature_schema_digest: schema_digest.to_string(),
        feature_names: BoundedVec::new(names).map_err(invalid)?,
        scale: QuantScaleTag::Q32,
        items: BoundedVec::new(items).map_err(invalid)?,
        synthetic,
    })
}

fn dispatch(
    ctx: &ExecutionContext,
    reader: &LogReader,
    op: DecisionLogOp,
    inputs: Option<&super::stat_learning::LearningInputs>,
    index: &medulla_decision_memory::memory::capability_index::CapabilityIndex,
    replay_graph: Option<&medulla_graph_core::graph::GraphView>,
    graphs: &GraphSnapshots,
) -> Result<ResultPayload, String> {
    use medulla_protocol_types::{
        result_contract::coordination::DecisionLogAggregate,
        result_contract::coordination::DecisionLogCommit,
        result_contract::coordination::DecisionLogCompact,
        result_contract::coordination::DecisionLogEvaluate,
        result_contract::coordination::DecisionLogGet,
        result_contract::coordination::DecisionLogResolve,
        result_contract::coordination::DecisionLogVerify,
    };
    match op {
        DecisionLogOp::Commit { record, evaluator } => {
            ResultPayload::of::<DecisionLogCommit>(commit(
                ctx,
                reader,
                &record,
                evaluator.as_ref(),
                replay_graph,
                graphs,
            )?)
        }
        DecisionLogOp::Evaluate { evaluation, .. } => {
            ResultPayload::of::<DecisionLogEvaluate>(evaluate(ctx, reader, evaluation)?)
        }
        DecisionLogOp::Get { record_id, .. } => {
            ResultPayload::of::<DecisionLogGet>(visible_entry(ctx.store, reader, &record_id)?)
        }
        DecisionLogOp::Aggregate { request } => {
            ResultPayload::of::<DecisionLogAggregate>(aggregate_log(ctx.store, reader, &request)?)
        }
        DecisionLogOp::Compact { policy, limit, .. } => ResultPayload::of::<DecisionLogCompact>(
            compact(ctx, &reader.retention, &policy, limit)?,
        ),
        DecisionLogOp::Resolve { resolution, .. } => {
            ResultPayload::of::<DecisionLogResolve>(resolve(ctx, reader, resolution)?)
        }
        DecisionLogOp::PromoteRule {
            rule,
            resolution_ids,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogPromoteRule,
        >(super::stat_rule_promotion::promote(
            ctx,
            reader,
            rule,
            &resolution_ids,
        )?),
        DecisionLogOp::Learn { write, .. } => {
            ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::DecisionLogLearn,
            >(super::stat_learning::dispatch(ctx, reader, write, inputs)?)
        }
        DecisionLogOp::Writeback {
            tenant_id,
            computation,
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogWriteback,
        >(super::stat_writeback::serve(
            ctx,
            reader,
            &tenant_id,
            computation,
        )?),
        DecisionLogOp::RunRecord { .. } => {
            return Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "a run record is staged by `run_record::stage` and its graph link is committed \
                 through the durable mutation path",
            ))
        }
        DecisionLogOp::CapabilityOutcome {
            entity_id,
            success,
            reward_q32,
            decision_id,
            source_ids,
            policy,
            ..
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogCapabilityOutcome,
        >(super::stat_capability_outcome::apply(
            ctx,
            index,
            &entity_id,
            success,
            reward_q32,
            &decision_id,
            &source_ids,
            &policy,
        )?),
        DecisionLogOp::GenerationSpace { .. } => {
            Err("generation_space is served on the async path".to_string())
        }
        DecisionLogOp::WritebackAudit {
            tenant_id,
            change_set_id,
        } => ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogWritebackAudit,
        >(super::stat_writeback::audit_rows(
            ctx,
            reader,
            &tenant_id,
            &change_set_id,
        )?),
        DecisionLogOp::HarnessAdoption { write, .. } => {
            ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::DecisionLogHarnessAdoption,
            >(super::stat_harness_adoption::adopt(ctx, reader, &write)?)
        }
        DecisionLogOp::HarnessLessons { query, .. } => {
            ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::DecisionLogHarnessLessons,
            >(super::stat_harness_adoption::lessons(ctx, &query)?)
        }
        DecisionLogOp::Verify { record_id, .. } => ResultPayload::of::<DecisionLogVerify>(verify(
            ctx,
            reader,
            &reader.retention,
            &record_id,
        )?),
    }
}

/// The caller's filtered snapshot of every graph a committed record read rows
/// from, read before the blocking commit; empty for every other op.
async fn commit_graphs(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &DecisionLogOp,
) -> Result<GraphSnapshots, String> {
    let mut graphs = GraphSnapshots::new();
    let DecisionLogOp::Commit { record, .. } = op else {
        return Ok(graphs);
    };
    for row in record.inputs.graph_rows.iter() {
        if !graphs.contains_key(&row.graph) {
            let snapshot = super::stat_support::filtered_view_versioned(
                state,
                verified.agent_id(),
                &row.graph,
            )
            .await?;
            graphs.insert(row.graph.clone(), snapshot);
        }
    }
    Ok(graphs)
}

async fn serve(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: DecisionLogOp,
) -> Result<ResultPayload, String> {
    if op.tenant_id() != verified.tenant() {
        return Err(
            "ACCESS_DENIED: DecisionLog tenant must match the verified request tenant".to_string(),
        );
    }
    let (store, secret) = {
        let mut guard = state.write().await;
        (guard.ensure_agent_library()?, guard.auth_secret.clone())
    };
    if let DecisionLogOp::GenerationSpace { logical, model, .. } = &op {
        let tenant = CarrierAuthority::from_verified(verified)?
            .tenant_scope()
            .to_string();
        let read =
            super::stat_generation_space::serve(state, &store, &tenant, logical, model.as_ref())
                .await?;
        return ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::DecisionLogGenerationSpace,
        >(read);
    }
    let reader = LogReader::served(state, verified).await;
    let index = super::stat_designation::shared_capability_index(&*state.read().await);
    let inputs = super::stat_learning::prepare(state, verified, &op).await?;
    let run_graph = super::run_record::open_graph(state, verified, &op).await?;
    // A committed family answer that re-reads a graph replays over the same
    // filtered view the served `Decide` opened.
    let replay_graph = match &op {
        DecisionLogOp::Commit { record, .. } => replay_view(state, verified, record).await?,
        _ => None,
    };
    let blocking_graph = run_graph.clone();
    let principal = verified.agent_id().to_string();
    let graphs = commit_graphs(state, verified, &op).await?;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let ctx = ExecutionContext {
            store: store.as_ref(),
            tenant_id: &reader.tenant_id,
            now_ms,
            server_secret: secret.as_bytes(),
            matched: &[],
            designation: None,
            reader: None,
        };
        match op {
            DecisionLogOp::RunRecord { record, .. } => super::run_record::stage(
                &ctx,
                &reader.retention,
                record,
                blocking_graph.as_deref(),
                &principal,
                &reader.principal,
            )
            .map(|staged| Served::RunRecord(Box::new(staged))),
            op => dispatch(
                &ctx,
                &reader,
                op,
                inputs.as_ref(),
                &index,
                replay_graph.as_ref(),
                &graphs,
            )
            .map(Served::Payload),
        }
    })
    .await
    .map_err(|error| format!("DecisionLog task failed: {error}"))?;
    let result = match result {
        Ok(Served::Payload(payload)) => Ok(payload),
        Ok(Served::RunRecord(staged)) => commit_run_record(state, req_id, verified, *staged).await,
        Err(error) => Err(error),
    };
    telemetry::logged(result.is_ok(), started);
    result
}

/// What the blocking half of a `DecisionLog` op produced.
enum Served {
    Payload(ResultPayload),
    /// A run record whose graph link commits through the durable mutation path
    /// before the answer is returned.
    RunRecord(Box<super::run_record::Staged>),
}

/// Commit a staged run record's graph link (the program node and its
/// `compiled_from` edge) through the MutationBatch commit used for every other
/// graph fact. With no durable backend the commit is refused.
async fn commit_run_record(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    staged: super::run_record::Staged,
) -> Result<ResultPayload, String> {
    if let Some(link) = staged.link {
        let persistence = state.read().await.persistence.clone();
        let batch_id = medulla_server_api::batch_compile::opaque_request_key(
            "run-record",
            &link.graph,
            req_id,
            &link.methods[0],
        );
        medulla_server_mutation::mutation_batch::commit_internal_graph_methods_with_nonce(
            medulla_server_mutation::mutation_batch::InternalGraphCommitRequest::new(
                persistence.as_ref(),
                &link.core,
                medulla_server_mutation::mutation_batch::CommitOrigin {
                    request_id: req_id,
                    principal: Some(verified.agent_id()),
                },
                &link.graph,
                &batch_id,
                link.methods,
                &ResultPayload::Bool(true),
            ),
        )
        .await
        .map_err(|error| {
            // `PERSISTENCE_UNAVAILABLE` is no declared code, so the wire hid every
            // cause as INTERNAL: keep a coded cause's own code, else report the
            // declared engine-unavailable code.
            medulla_protocol_types::error_registry::classify_refusal(
                "ENGINE_UNAVAILABLE",
                "run record link not committed: ",
                &error,
            )
        })?;
    }
    ResultPayload::of::<medulla_protocol_types::result_contract::coordination::DecisionLogRunRecord>(
        staged.stored,
    )
}

/// Serve one `DecisionLog` op.
pub(super) async fn handle_log(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: DecisionLogOp,
) -> Response {
    let span = telemetry::span("DecisionLog", verified.tenant());
    match serve(state, req_id, verified, op).instrument(span).await {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => {
            telemetry::refused("DecisionLog", &error);
            Response::err(req_id, error)
        }
    }
}

/// The numeric fact a route option's observed cost is filed under (R039.2).
pub(super) const OBSERVED_COST_KEY: &str = "l5.observed_cost";

/// Fill each option's `l5.observed_cost` number from the tenant's usage
/// ledger (R039.2, R801): the mean derived cost per call of the model the
/// option names, in micro-units on the `Q32` scale. The ledger is keyed by
/// the verified tenant. An option whose model has no ledger row, or a row no
/// price covers, gets no number: unknown is never read as zero.
pub(super) fn fill_observed_costs(
    store: &AgentLibraryStore,
    tenant_id: &str,
    views: &mut [CandidateView],
) -> Result<(), String> {
    if views.is_empty() {
        return Ok(());
    }
    let refused = |error: medulla_persistence_records::usage_ledger::UsageRefusal| {
        refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("the usage ledger refused the read: {error:?}"),
        )
    };
    let catalog = medulla_types::usage::PriceCatalog {
        aliases: BTreeMap::new(),
        entries: store.price_entries(tenant_id).map_err(refused)?,
    };
    let rollup = store
        .usage_rollup(tenant_id, &catalog, 0, u64::MAX)
        .map_err(refused)?;
    // Per model: calls and the summed cost; `None` once any day has no price.
    let mut per_model: BTreeMap<&str, (u64, Option<u64>)> = BTreeMap::new();
    for day in &rollup.days {
        let slot = per_model
            .entry(day.model_key.as_str())
            .or_insert((0, Some(0)));
        slot.0 = slot.0.saturating_add(day.calls);
        slot.1 = slot
            .1
            .and_then(|sum| day.cost_micro.and_then(|cost| sum.checked_add(cost)));
    }
    for view in views.iter_mut() {
        let key =
            medulla_persistence_records::usage_ledger::normalize_model_key(&catalog, &view.id);
        let Some((calls, Some(sum))) = per_model.get(key.as_str()).copied() else {
            continue;
        };
        if calls == 0 {
            continue;
        }
        let micros = i64::try_from(sum / calls)
            .ok()
            .and_then(|mean| mean.checked_mul(1i64 << 32))
            .ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    format!(
                        "the observed cost of {} does not fit the Q32 scale",
                        view.id
                    ),
                )
            })?;
        view.numbers.insert(OBSERVED_COST_KEY.to_string(), micros);
    }
    Ok(())
}

/// Fill every `OutcomeRate` feature's numeric fact from the log's pooled
/// rates (§6.1 outcome statistics, EH-063). Those rates summarise other
/// principals' runs, so they are readable only when the policy declares
/// cross-principal features tenant-public; an option below `min_support`
/// gets no fact and falls to the feature's declared missing-value rule.
pub(super) fn fill_outcome_rates(
    store: &AgentLibraryStore,
    reader: &LogReader,
    schema: &FeatureSchemaBody,
    tenant_public: bool,
    views: &mut [CandidateView],
) -> Result<(), String> {
    for spec in &schema.features {
        let FeatureKind::OutcomeRate { question_id } = &spec.kind else {
            continue;
        };
        if !tenant_public {
            return Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "an outcome-rate feature reads other principals' runs; the policy must declare \
                 tenant_public_features",
            ));
        }
        let request = OutcomeAggregateRequest {
            tenant_id: reader.tenant_id.clone(),
            question_id: Some(question_id.clone()),
            window: RecordWindow {
                from_ms: 0,
                to_ms: u64::MAX,
            },
            attribution: None,
            transfer: None,
        };
        let key = outcome_rate_key(question_id);
        let mut best: BTreeMap<String, (u64, i64)> = BTreeMap::new();
        for row in aggregate_log(store, reader, &request)?.rows.iter() {
            let Some(rate) = row.pooled_rate else {
                continue;
            };
            let slot = best.entry(row.option_id.clone()).or_insert((0, rate.value));
            if row.trials > slot.0 {
                *slot = (row.trials, rate.value);
            }
        }
        for view in views.iter_mut() {
            if let Some((_, value)) = best.get(&view.id) {
                view.numbers.insert(key.clone(), *value);
            }
        }
    }
    Ok(())
}
