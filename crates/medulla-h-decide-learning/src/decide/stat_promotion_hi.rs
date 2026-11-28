//! The held-out evidence and the correction evidence of a promotion, split out above the generation and the corrections they read.

use medulla_persistence_records::decision_jobs::read_pointer;
use {
    medulla_decision_policy_types::decision::held_out::DefinitionKind,
    medulla_decision_policy_types::decision::held_out::GraderKind,
    medulla_decision_policy_types::decision::held_out::HeldOutEvidence,
    medulla_decision_policy_types::decision::held_out::ReceiptKind,
    medulla_decision_policy_types::decision::held_out::RevisionSubject,
    medulla_decision_policy_types::decision::held_out::HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use {
    medulla_h_decide_base::decide::stat_promotion::bundle_evidence,
    medulla_h_decide_base::decide::stat_promotion::correction_set_digest,
    medulla_h_decide_base::decide::stat_promotion::decision_evidence,
    medulla_h_decide_base::decide::stat_promotion::mean_micros,
    medulla_h_decide_base::decide::stat_promotion::policy_evidence,
    medulla_h_decide_base::decide::stat_promotion::program_evidence,
    medulla_h_decide_base::decide::stat_promotion::rollout_evidence,
    medulla_h_decide_base::decide::stat_promotion::unevaluated,
    medulla_h_decide_base::decide::stat_promotion::EvidenceError,
    medulla_h_decide_base::decide::stat_promotion::EvidenceSources,
};

/// The held-out evidence `receipt_digest` names for `subject`. A receipt of
/// another tenant resolves as absent. Writes nothing.
pub fn held_out_evidence(
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
        // A skill revision is promoted on the paired rollouts of its challenger (R712.3).
        // A rule candidate is promoted on its independent correction outcomes (R060, R702.8).
        RevisionSubject::Definition {
            definition_kind: DefinitionKind::Rule,
            definition_ref,
        } => correction_evidence(sources, subject, definition_ref, receipt_digest),
        RevisionSubject::Definition { .. }
        | RevisionSubject::RetrievalStrategy { .. }
        | RevisionSubject::Skill { .. } => rollout_evidence(sources, subject, receipt_digest),
        RevisionSubject::KnobSet { .. } | RevisionSubject::ModelPolicy { .. } => {
            policy_evidence(sources, subject, receipt_digest)
        }
        // A policy scope resolves through its recorded review (MEDULLA-ORCH-VOCAB-R012.7.3).
        RevisionSubject::GovernedSet { scope_ref } => {
            match medulla_h_decide_runlog::decide::policy_activation::review_evidence(
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

/// The evidence of a rule candidate, `Definition { Rule, <rule_id> }` (R060). The one read of
/// `CorrectionOutcome` nodes is `independent_outcomes` (R702.8); this adds no second read.
/// Only outcomes tagged `independent` are read, so the evidence is independent exactly when it
/// exists. The score is the mean outcome value; the champion (when the pointer names a rule)
/// scores its own mean over its independent outcomes, and without one there is no baseline.
/// `receipt_digest` must be [`correction_set_digest`] of what the graph shows now.
pub(super) fn correction_evidence(
    sources: &EvidenceSources,
    subject: &RevisionSubject,
    rule_id: &str,
    receipt_digest: &str,
) -> Result<HeldOutEvidence, EvidenceError> {
    let graph = sources.graph.ok_or_else(unevaluated)?;
    let challenger = super::correction::independent_outcomes(graph, sources.tenant, Some(rule_id))
        .into_iter()
        .find(|candidate| candidate.rule_id == rule_id && candidate.independent > 0)
        .ok_or_else(unevaluated)?;
    let challenger_ppm = mean_micros(challenger.mean_value);
    let set_digest = correction_set_digest(rule_id, challenger.independent, challenger_ppm);
    if set_digest != receipt_digest {
        return Err(unevaluated());
    }
    let (_, state) = read_pointer(sources.store, sources.tenant, &subject.pointer_key())?;
    let champion_ref = state.target().map(str::to_owned);
    let champion_score = match &champion_ref {
        Some(champion) => {
            super::correction::independent_outcomes(graph, sources.tenant, Some(champion.as_str()))
                .into_iter()
                .find(|candidate| &candidate.rule_id == champion && candidate.independent > 0)
                .map(|candidate| mean_micros(candidate.mean_value))
        }
        None => None,
    };
    Ok(HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: subject.clone(),
        challenger_ref: rule_id.to_owned(),
        champion_ref: champion_ref.filter(|_| champion_score.is_some()),
        receipt_kind: ReceiptKind::CorrectionOutcomes,
        receipt_digest: set_digest.clone(),
        evaluation_set_digest: set_digest,
        challenger_score_ppm: challenger_ppm,
        champion_score_ppm: champion_score,
        delta_ppm: champion_score.map_or(0, |champion| challenger_ppm - champion),
        case_count: challenger.independent,
        independent: true,
        grader_kind: GraderKind::Human,
        measurement: None,
        review: None,
    })
}
