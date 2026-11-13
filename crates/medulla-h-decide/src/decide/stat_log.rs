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
use super::stat_replay::replay;
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
) -> Result<DecisionLogCommitted, String> {
    if record.caller_principal != reader.principal {
        return Err(
            "ACCESS_DENIED: a decision is logged by the principal that made it".to_string(),
        );
    }
    let logged = replay(ctx, record)?;
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
    use medulla_decision_stats::decision::aggregate::{label_use, LabelUse};
    let floor = default_statistical_policy().min_outcome_fidelity;
    let mut labels: Vec<(u64, String, bool)> = Vec::new();
    for (entry, evaluations) in joined(store, reader, None, None)? {
        let record = &entry.record;
        if record.synthetic_evidence || executed_option(&record.outcome) != Some(option_id) {
            continue;
        }
        for stored in &evaluations {
            if let LabelUse::Label(success) = label_use(stored, &record.caller_principal, floor) {
                labels.push((
                    stored.recorded_at_ms,
                    stored.evaluation.evaluation_id.clone(),
                    success,
                ));
            }
        }
    }
    labels.sort_by(|a, b| (b.0, &b.1).cmp(&(a.0, &a.1)));
    Ok(labels.into_iter().take(n).map(|(_, _, ok)| ok).collect())
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
        DecisionLogOp::Commit { record, evaluator } => ResultPayload::of::<DecisionLogCommit>(
            commit(ctx, reader, &record, evaluator.as_ref())?,
        ),
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
        DecisionLogOp::Verify { record_id, .. } => ResultPayload::of::<DecisionLogVerify>(verify(
            ctx,
            reader,
            &reader.retention,
            &record_id,
        )?),
    }
}

async fn serve(
    state: &SharedState,
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
    let reader = LogReader::served(state, verified).await;
    let inputs = super::stat_learning::prepare(state, verified, &op).await?;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let ctx = ExecutionContext {
            store: store.as_ref(),
            tenant_id: &reader.tenant_id,
            now_ms,
            server_secret: secret.as_bytes(),
            matched: &[],
        };
        dispatch(&ctx, &reader, op, inputs.as_ref())
    })
    .await
    .map_err(|error| format!("DecisionLog task failed: {error}"))?;
    telemetry::logged(result.is_ok(), started);
    result
}

/// Serve one `DecisionLog` op.
pub(super) async fn handle_log(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: DecisionLogOp,
) -> Response {
    let span = telemetry::span("DecisionLog", verified.tenant());
    match serve(state, verified, op).instrument(span).await {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => {
            telemetry::refused("DecisionLog", &error);
            Response::err(req_id, error)
        }
    }
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
