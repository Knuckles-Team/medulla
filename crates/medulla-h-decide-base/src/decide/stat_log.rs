//! `DecisionLog`, served (EH-060, EH-061, EH-012), and the logged-dataset
//! builder `DecisionFit`/`DecisionEval` read labels from.
//!
//! Every read filters by each record's own visibility before a record, an
//! evaluation, a count or a rate is formed: tenant-wide for library-sourced
//! records, the committing principal only for graph-sourced ones.

use super::stat_executor::ExecutionContext;
use super::stat_replay_lo::GraphSnapshots;
use super::stat_retention::full_record;
use super::stat_slate::{evaluated_slates, slate_of, SLATE_QUESTION};
use super::stat_support::{default_statistical_policy, pinned_entry, refusal};
use super::SharedState;
use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_stats::decision::features::outcome_rate_key;
use medulla_decision_stats::decision::transfer::{
    robust_transfer, skill_candidates, SkillKnobs, SkillRow, TransferKnobs, TransferRow,
};
use medulla_decision_stats::{
    decision::aggregate::aggregate, decision::aggregate::AggregateRules,
    decision::aggregate::JoinedRecord,
};
use medulla_decision_stats_records::decision::candidate::CandidateView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::evaluation_key;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;
use std::collections::BTreeMap;
use {
    super::stat_log_reader::store_once, super::stat_log_reader::visible_entries,
    super::stat_log_reader::LogReader, super::stat_log_reader::MatchedTrajectory,
    super::stat_log_reader::MAX_LOG_ROWS,
};
use {
    medulla_decision_policy_types::decision::statistical::dataset::ItemLabel,
    medulla_decision_policy_types::decision::statistical::dataset::LabelledDataset,
    medulla_decision_policy_types::decision::statistical::dataset::LabelledItem,
    medulla_decision_policy_types::decision::statistical::dataset::LoggedOutcome,
    medulla_decision_policy_types::decision::statistical::dataset::OutcomeEvaluation,
    medulla_decision_policy_types::decision::statistical::dataset::PropensitySource,
    medulla_decision_policy_types::decision::statistical::dataset::LABELLED_DATASET_SCHEMA_VERSION,
};
use {
    medulla_decision_policy_types::decision::transfer::is_digest,
    medulla_decision_policy_types::decision::transfer::TransferEstimate,
    medulla_decision_policy_types::decision::transfer::TransferRequest,
};
use {
    medulla_decision_program_types::decision::statistical::features::FeatureKind,
    medulla_decision_program_types::decision::statistical::features::FeatureSchemaBody,
};
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_types::decision::statistical::FeatureMatrixRef,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};
use {
    medulla_decision_statistical_types::decision::QuantScaleTag,
    medulla_decision_types::decision::jobs::RecordWindow,
    medulla_decision_types::decision::record::CandidateSourceRecord,
};
use {
    medulla_decision_types::decision::statistical::log::CorrectionLabel,
    medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    medulla_decision_types::decision::statistical::log::DecisionLogOp,
    medulla_decision_types::decision::statistical::log::DecisionOutcomeEvaluation,
    medulla_decision_types::decision::statistical::log::OutcomeAggregate,
    medulla_decision_types::decision::statistical::log::OutcomeAggregateRequest,
    medulla_decision_types::decision::statistical::log::RecordVisibility,
    medulla_decision_types::decision::statistical::log::StoredEvaluation,
    medulla_decision_types::decision::statistical::log::DECISION_LOG_SCHEMA_VERSION,
};

pub fn executed_option(outcome: &StatisticalOutcome) -> Option<&str> {
    match outcome {
        StatisticalOutcome::Acted { option_id, .. }
        | StatisticalOutcome::Explored { option_id, .. } => Some(option_id),
        StatisticalOutcome::Advisory { .. } | StatisticalOutcome::Abstained { .. } => None,
    }
}

/// The digest two decisions share exactly when they ask the same question
/// with the same typed parameters (MEDULLA-DECISION-ENGINE-R020.1.1).
pub fn context_digest_of(
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
pub struct MatchWindow {
    pub now_ms: u64,
    pub window_ms: u64,
    pub max_matched: usize,
}

/// Executed, committed records of the reader's own tenant AND principal whose
/// sealed `context_digest` equals `context_digest`, inside `window`, newest
/// first up to `max_matched` and returned in record id order. Another
/// principal's or tenant's entries are never read.
pub fn matched_trajectories(
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
pub fn latest_disclosure(
    store: &AgentLibraryStore,
    reader: &LogReader,
    set_id: &str,
    scan_limit: usize,
) -> Result<Option<medulla_decision_types::decision::instruction_disclosure::DisclosureEvent>, String>
{
    use medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind;
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

pub fn evaluations_of(
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

pub fn visibility_of(record: &StatisticalDecisionRecord, principal: &str) -> RecordVisibility {
    match record.candidate_source {
        CandidateSourceRecord::AgentLibrary { .. } => RecordVisibility::Tenant,
        CandidateSourceRecord::Graph { .. } | CandidateSourceRecord::Declared { .. } => {
            RecordVisibility::Principal {
                principal: principal.to_string(),
            }
        }
    }
}

/// The program a committed `skill_workflow_compile` record stores, in the transaction that
/// stores the record (MEDULLA-DECISION-ENGINE-R156.3): under the key `ProgramLookup` reads, so
/// the same path as a program compiled from a run (R135.3). A program already stored under the
/// skill document's signature with the same content stores nothing again; other content under
/// it is a conflict, never an overwrite.
pub fn skill_program_rows(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
    use {
        medulla_decision_program_types::decision::program_store::program_key,
        medulla_decision_program_types::decision::program_store::CompiledProgram,
    };
    let Some(FamilyAnswer::SkillWorkflow(program)) = &record.answer else {
        return Ok(Vec::new());
    };
    program.validate().map_err(|error| {
        refusal(
            StatisticalErrorCode::ParameterInvalid,
            format!("the compiled skill program is invalid: {error:?}"),
        )
    })?;
    let key = program_key(&program.task_signature);
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let existing: CompiledProgram = decode_artifact(&bytes, "compiled program")?;
        if existing == **program {
            return Ok(Vec::new());
        }
        return Err(refusal(
            StatisticalErrorCode::IdempotencyConflict,
            "a program is stored under this skill document with other content",
        ));
    }
    Ok(vec![(key, encode_artifact(&**program)?)])
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

pub fn evaluate(
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

/// Visible, executed records of `question_id` (or of every question) inside
/// the window, each with its evaluations.
pub fn joined(
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
pub fn aggregate_log(
    store: &AgentLibraryStore,
    reader: &LogReader,
    request: &OutcomeAggregateRequest,
    correction_labels: &[CorrectionLabel],
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
        correction_labels: BoundedVec::new(correction_labels.to_vec())
            .map_err(|_| "the correction labels exceed the aggregate bound".to_string())?,
    })
}

/// The latest `n` outcomes of `option_id` that count as labels (independent,
/// observed, uncensored, at the fidelity floor), newest first; `true` is a
/// success. Synthetic records never count. This is the skip rule's input
/// (MEDULLA-DECISION-ENGINE-R159.3): it reads the same label rule as
/// [`aggregate_log`].
pub fn recent_evaluations(
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
pub struct OutcomeLabel {
    /// The option the reviewed decision executed.
    pub option_id: String,
    pub recorded_at_ms: u64,
    pub evaluation_id: String,
    pub success: bool,
}

/// The latest `n` labels of `option_id`, newest first, under the same label
/// rule as [`recent_evaluations`] (MEDULLA-DECISION-ENGINE-R159.5).
pub fn recent_labels(
    store: &AgentLibraryStore,
    reader: &LogReader,
    option_id: &str,
    n: usize,
) -> Result<Vec<OutcomeLabel>, String> {
    recent_labels_where(store, reader, |executed| executed == option_id, n)
}

/// The latest `n` labels of every option `wanted` accepts, newest first, under
/// the one label rule (MEDULLA-DECISION-ENGINE-R706.3).
pub fn recent_labels_where(
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
pub struct RecentLabels {
    /// The latest labels, newest first; `true` is a success.
    pub labels: Vec<bool>,
    /// Evaluations of the option that are refused as labels.
    pub refused: usize,
}

/// The latest `n` labels of `option_id` and how many of its evaluations were
/// refused as labels.
pub fn recent_labels_counted(
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
    fidelity_floor: medulla_decision_statistical_types::decision::TraceFidelityLevel,
) -> Result<
    BoundedVec<medulla_decision_policy_types::decision::transfer::SkillCandidateRow, 1024>,
    String,
> {
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
    fidelity_floor: medulla_decision_statistical_types::decision::TraceFidelityLevel,
) -> Result<TransferEstimate, String> {
    use {
        medulla_decision_statistical_types::decision::record::premise::PremiseClass,
        medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
        medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    };
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
        medulla_types::evidence_class::EvidenceClass::Proof => PremiseClass::Proof,
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
        trust_tier: None,
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
        trust_tier: None,
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
pub fn logged_dataset(
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

/// The graph reads a `DecisionLog` op carries into its blocking half, all
/// opened before it.
pub struct ReadGraphs<'a> {
    /// The view a committed family answer re-reads.
    pub replay: Option<&'a medulla_graph_core::graph::GraphView>,
    /// The graph rows a commit re-reads at their versions.
    pub snapshots: &'a GraphSnapshots,
    /// The correction labels an `Aggregate` joins (R059); empty when it asked for none.
    pub correction_labels: &'a [CorrectionLabel],
    /// The graphs a `Provenance` read follows action links through.
    pub provenance: &'a [(String, std::sync::Arc<medulla_graph_core::graph::GraphCore>)],
}

/// Every graph a committed record read rows from, as it stood at the version
/// each row was read at, read before the blocking commit; empty for every other
/// op.
pub async fn commit_graphs(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &DecisionLogOp,
) -> Result<GraphSnapshots, String> {
    let DecisionLogOp::Commit { record, .. } = op else {
        return Ok(GraphSnapshots::new());
    };
    super::stat_support::historical_views(
        state,
        verified.agent_id(),
        record.inputs.graph_rows.as_slice(),
    )
    .await
}

/// What the blocking half of a `DecisionLog` op produced.
pub enum Served {
    Payload(ResultPayload),
    /// A run record whose graph link commits through the durable mutation path
    /// before the answer is returned.
    RunRecord(Box<super::run_record::Staged>),
}

/// Commit the graph writes an op staged (`what` names them in the refusal), through the same
/// MutationBatch commit used for every other graph fact. The embedding writes reach the drift
/// monitors, so the next drift refresh sees the region. With no durable backend the commit is
/// refused.
pub async fn commit_staged_link(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    link: super::run_record::StagedLink,
    what: &str,
) -> Result<(), String> {
    let persistence = state.read().await.persistence.clone();
    let batch_id = medulla_server_api::batch_compile::opaque_request_key(
        what,
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
    .map(|_| ())
    .map_err(|error| {
        medulla_protocol_types::error_registry::classify_refusal(
            "ENGINE_UNAVAILABLE",
            &format!("{what} rows not committed to the graph: "),
            &error,
        )
    })
}

/// The `LimitBreachObservation` nodes a committed abstention stages
/// (MEDULLA-DECISION-ENGINE-R131.5): one node `breach:<record_id>:<rule_id>` per breach with its
/// edge `eg:breachOf` to the signal, in the graph the record's `limit.graph` param names. A
/// node that already exists is not written again, so a retry of the same record adds none; an
/// absent signal node is created. `None` when the record holds no breach or names no graph.
pub async fn breach_link(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &DecisionLogOp,
) -> Result<Option<super::run_record::StagedLink>, String> {
    use medulla_decision_types::decision::record::AbstainReason;
    use medulla_decision_types::decision::statistical::TypedValue;
    use medulla_protocol_types::protocol::Method;
    let DecisionLogOp::Commit { record, .. } = op else {
        return Ok(None);
    };
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        return Ok(None);
    };
    let Some(graph) = record
        .inputs
        .params
        .iter()
        .find_map(|param| match &param.value {
            TypedValue::Text(graph) if param.name == "limit.graph" => Some(graph.clone()),
            _ => None,
        })
    else {
        return Ok(None);
    };
    let (core, _isolation) = super::stat_support::graph_at_level(
        state,
        verified.agent_id(),
        &graph,
        StatisticalErrorCode::ParameterInvalid,
        medulla_isolation::isolation::AccessLevel::Write,
    )
    .await?;
    let encode = |value: serde_json::Value| {
        rmp_serde::to_vec_named(&value).map_err(|error| {
            refusal(
                StatisticalErrorCode::ParameterInvalid,
                format!("limit breach encoding failed: {error}"),
            )
        })
    };
    let mut methods = Vec::new();
    let mut signals: Vec<&str> = Vec::new();
    for reason in reasons.iter() {
        let AbstainReason::LimitBreach {
            rule_id,
            observed,
            bound,
            signal_iri,
        } = reason
        else {
            continue;
        };
        let node_id = format!("breach:{}:{rule_id}", record.record_id);
        if core.get_node_properties(&node_id).is_some() {
            continue;
        }
        if core.get_node_properties(signal_iri).is_none() && !signals.contains(&signal_iri.as_str())
        {
            signals.push(signal_iri);
            methods.push(Method::AddNode {
                node_id: signal_iri.clone(),
                properties_msgpack: encode(serde_json::json!({
                    "type": "Signal",
                    "_owner": verified.agent_id(),
                }))?,
            });
        }
        methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(serde_json::json!({
                "type": "LimitBreachObservation",
                "rule_id": rule_id,
                "observed": observed,
                "bound": bound,
                "record_id": record.record_id,
                "_owner": verified.agent_id(),
            }))?,
        });
        methods.push(Method::AddEdge {
            source_id: node_id,
            target_id: signal_iri.clone(),
            properties_msgpack: encode(serde_json::json!({
                "relationship": "eg:breachOf",
                "tenant": record.tenant_id,
            }))?,
        });
    }
    Ok(
        (!methods.is_empty()).then_some(super::run_record::StagedLink {
            graph,
            core,
            methods,
        }),
    )
}

/// Commit a staged run record's graph link (the program node and its
/// `compiled_from` edge) through the MutationBatch commit used for every other
/// graph fact. With no durable backend the commit is refused.
pub async fn commit_run_record(
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

/// The numeric fact a route option's observed cost is filed under (R039.2).
pub const OBSERVED_COST_KEY: &str = "l5.observed_cost";

/// Fill each option's `l5.observed_cost` number from the tenant's usage
/// ledger (R039.2, R801): the mean derived cost per call of the model the
/// option names, in micro-units on the `Q32` scale. The ledger is keyed by
/// the verified tenant. An option whose model has no ledger row, or a row no
/// price covers, gets no number: unknown is never read as zero.
pub fn fill_observed_costs(
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
    let catalog = medulla_decision_policy_types::usage::PriceCatalog {
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
pub fn fill_outcome_rates(
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
            corrections: None,
            transfer: None,
        };
        let key = outcome_rate_key(question_id);
        let mut best: BTreeMap<String, (u64, i64)> = BTreeMap::new();
        for row in aggregate_log(store, reader, &request, &[])?.rows.iter() {
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
