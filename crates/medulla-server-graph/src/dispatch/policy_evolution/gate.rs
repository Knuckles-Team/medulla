//! Cross-record admission: the rules a policy-evolution record must satisfy
//! against what the graph already holds.
//!
//! Pure over [`RecordLookup`], so every refusal is unit-testable without a
//! server. Structural checks on one body already ran in `medulla-types`.

use medulla_decision_types::decision::held_out::RevisionSubject;
use medulla_decision_outcome_types::policy_evolution::{
    CaptureEligibility, ModelPolicyVersion, OpenWeightPolicyCapability, PolicyCapture,
    PolicyEvaluation, PolicyEvolutionRecord, PolicyEvolutionRecordBody, PolicyRefusal, TrainingRun,
    TrainingRunOutcome, VersionOrigin,
};

/// What admission may read from the request graph.
pub trait RecordLookup {
    /// The verified record under `record_id`, `None` when absent. A stored
    /// record whose body no longer matches its id is a refusal, never a value.
    fn record(&self, record_id: &str) -> Result<Option<PolicyEvolutionRecord>, PolicyRefusal>;
    /// The step count of a `:Trajectory` node, `None` when there is none.
    fn trajectory_steps(&self, trajectory_id: &str) -> Option<u64>;
    /// The content digests of the published `DecisionPolicy` component
    /// `policy_id`, one per retained revision; empty when it was never
    /// published or is another kind of component.
    fn policy_component_digests(&self, policy_id: &str) -> Result<Vec<String>, PolicyRefusal>;
}

/// Admit `record` against the stored graph, or answer the typed refusal.
pub fn admit(
    record: &PolicyEvolutionRecord,
    lookup: &dyn RecordLookup,
) -> Result<(), PolicyRefusal> {
    match record {
        PolicyEvolutionRecord::Capability { .. } => Ok(()),
        PolicyEvolutionRecord::Capture { record } => admit_capture(record, lookup),
        PolicyEvolutionRecord::ModelPolicyVersion { record } => admit_version(record, lookup),
        PolicyEvolutionRecord::TrainingRun { record } => admit_training_run(record, lookup),
        PolicyEvolutionRecord::PolicyEvaluation { record } => admit_evaluation(record, lookup),
    }
}

/// The stored body of kind `T` under `record_id`; absent or of another kind
/// answers `missing`.
fn fetch<T: PolicyEvolutionRecordBody>(
    lookup: &dyn RecordLookup,
    record_id: &str,
    missing: fn(&str) -> PolicyRefusal,
) -> Result<T, PolicyRefusal> {
    lookup
        .record(record_id)?
        .and_then(T::from_record)
        .ok_or_else(|| missing(record_id))
}

fn record_missing(record_id: &str) -> PolicyRefusal {
    PolicyRefusal::RecordMissing(record_id.to_string())
}

fn capability_missing(_: &str) -> PolicyRefusal {
    PolicyRefusal::CapabilityMissing
}

/// A capture is admitted only under an attested capability whose `capture`
/// control is on, sampled by the version that capability serves, over a
/// trajectory that exists with exactly the step count it claims.
fn admit_capture(capture: &PolicyCapture, lookup: &dyn RecordLookup) -> Result<(), PolicyRefusal> {
    let capability: OpenWeightPolicyCapability =
        fetch(lookup, capture.capability_id.as_str(), capability_missing)?;
    if !capability.controls.capture.enabled {
        return Err(PolicyRefusal::CaptureDisabled);
    }
    let sampler: ModelPolicyVersion =
        fetch(lookup, capture.sampler_version_id.as_str(), record_missing)?;
    check_sampler(capture, &capability, &sampler)?;
    match lookup.trajectory_steps(capture.trajectory_id.as_str()) {
        None => Err(PolicyRefusal::TrajectoryMissing),
        Some(steps) if steps != u64::from(capture.trajectory_steps) => {
            Err(PolicyRefusal::TrajectoryLengthMismatch)
        }
        Some(_) => Ok(()),
    }
}

/// The sampler is the exact artifact the capability attested, and it reports
/// every record the capture carries.
fn check_sampler(
    capture: &PolicyCapture,
    capability: &OpenWeightPolicyCapability,
    sampler: &ModelPolicyVersion,
) -> Result<(), PolicyRefusal> {
    let same_artifact = sampler.checkpoint_digest == capability.base_checkpoint_digest
        && sampler.adapter_digest == capability.adapter_digest
        && sampler.tokenizer_digest == capability.tokenizer_digest;
    if !same_artifact {
        return Err(PolicyRefusal::SamplerMismatch);
    }
    let support = capability.logprobs;
    let unsupported_top_k = capture.sampler_top_k.is_some() && support.top_k == 0;
    let unsupported_draws = capture.auxiliary_draws.is_some() && !support.auxiliary_draws;
    if !support.chosen_token || unsupported_top_k || unsupported_draws {
        return Err(PolicyRefusal::LogprobsUnsupported);
    }
    Ok(())
}

/// A version's parent exists; a trained version is the one new adapter a
/// succeeded run produced on that parent. A failed or cancelled run has no
/// output, so it can never mint a servable version.
fn admit_version(
    version: &ModelPolicyVersion,
    lookup: &dyn RecordLookup,
) -> Result<(), PolicyRefusal> {
    if let Some(parent) = &version.parent_version_id {
        fetch::<ModelPolicyVersion>(lookup, parent.as_str(), record_missing)?;
    }
    let VersionOrigin::Trained { training_run_id } = &version.origin else {
        return Ok(());
    };
    let run: TrainingRun = fetch(lookup, training_run_id.as_str(), record_missing)?;
    let TrainingRunOutcome::Succeeded { output } = &run.outcome else {
        return Err(PolicyRefusal::RunNotSucceeded);
    };
    let produced = version.adapter_digest == Some(output.artifact_digest)
        && version.parent_version_id.as_ref() == Some(&run.base_version_id);
    if !produced {
        return Err(PolicyRefusal::InvalidRecord(
            "a trained version must be the run's output adapter on the run's base".into(),
        ));
    }
    Ok(())
}

/// A run is admitted only under a capability whose `train` control is on,
/// from an existing base version, over trainer-eligible captures.
fn admit_training_run(run: &TrainingRun, lookup: &dyn RecordLookup) -> Result<(), PolicyRefusal> {
    let capability: OpenWeightPolicyCapability =
        fetch(lookup, run.capability_id.as_str(), capability_missing)?;
    if !capability.controls.train.enabled {
        return Err(PolicyRefusal::TrainDisabled);
    }
    fetch::<ModelPolicyVersion>(lookup, run.base_version_id.as_str(), record_missing)?;
    for capture_id in run.input_capture_ids.iter() {
        let capture: PolicyCapture = fetch(lookup, capture_id.as_str(), record_missing)?;
        if capture.eligibility() != CaptureEligibility::Eligible {
            return Err(PolicyRefusal::CaptureIneligible(
                capture_id.as_str().to_string(),
            ));
        }
    }
    Ok(())
}

/// An evaluation names a revision that exists, and its held-out trajectories
/// are disjoint from the captures the evaluated version's training run read: a
/// version trained on a trajectory is never evaluated on it. The champion it is
/// compared with is held to the same split, so a comparison never favours a side
/// that saw the held-out set. A `ModelPolicy` revision is a `ModelPolicyVersion`
/// record; a `KnobSet` revision is a published `DecisionPolicy` component
/// revision, read through [`RecordLookup`]. A `DecisionHead` or `Program`
/// subject is not evaluated here.
fn admit_evaluation(
    evaluation: &PolicyEvaluation,
    lookup: &dyn RecordLookup,
) -> Result<(), PolicyRefusal> {
    let mut evaluated: Option<ModelPolicyVersion> = None;
    let mut champion: Option<ModelPolicyVersion> = None;
    match &evaluation.subject {
        RevisionSubject::ModelPolicy { .. } => {
            evaluated = Some(fetch(
                lookup,
                evaluation.revision_ref.as_str(),
                record_missing,
            )?);
            if let Some(champion_ref) = &evaluation.champion_revision_ref {
                champion = Some(fetch(lookup, champion_ref.as_str(), record_missing)?);
            }
        }
        RevisionSubject::KnobSet { policy_id } => {
            let published = lookup.policy_component_digests(policy_id)?;
            let revisions =
                std::iter::once(&evaluation.revision_ref).chain(&evaluation.champion_revision_ref);
            for revision_ref in revisions {
                if !published.iter().any(|digest| digest == revision_ref.as_str()) {
                    return Err(record_missing(revision_ref.as_str()));
                }
            }
        }
        RevisionSubject::DecisionHead { .. } | RevisionSubject::Program { .. } => {
            return Err(PolicyRefusal::SubjectUnsupported(
                subject_kind(&evaluation.subject).to_string(),
            ));
        }
        other => {
            return Err(PolicyRefusal::InvalidRecord(format!(
                "a policy evaluation cannot name a {} subject",
                subject_kind(other)
            )));
        }
    }
    if evaluation.held_out_trajectory_ids.is_empty() {
        return Ok(());
    }
    let held_out = |trajectory_id: &str| {
        evaluation
            .held_out_trajectory_ids
            .iter()
            .any(|held_out| held_out.as_str() == trajectory_id)
    };
    if let Some(overlap) = trained_on(evaluated.as_ref(), lookup)?
        .into_iter()
        .find(|trajectory_id| held_out(trajectory_id))
    {
        return Err(PolicyRefusal::EvaluationOverlapsTraining(overlap));
    }
    if let Some(overlap) = trained_on(champion.as_ref(), lookup)?
        .into_iter()
        .find(|trajectory_id| held_out(trajectory_id))
    {
        return Err(PolicyRefusal::SplitOverlap(overlap));
    }
    Ok(())
}

/// The snake_case kind of a subject, from its pointer key.
fn subject_kind(subject: &RevisionSubject) -> String {
    subject
        .pointer_key()
        .split(':')
        .nth(1)
        .unwrap_or("unknown")
        .to_string()
}

/// The trajectories the training run behind `version` read; empty for a base
/// version or none.
fn trained_on(
    version: Option<&ModelPolicyVersion>,
    lookup: &dyn RecordLookup,
) -> Result<Vec<String>, PolicyRefusal> {
    let Some(ModelPolicyVersion {
        origin: VersionOrigin::Trained { training_run_id },
        ..
    }) = version
    else {
        return Ok(Vec::new());
    };
    let run: TrainingRun = fetch(lookup, training_run_id.as_str(), record_missing)?;
    let mut trajectories = Vec::new();
    for capture_id in run.input_capture_ids.iter() {
        let capture: PolicyCapture = fetch(lookup, capture_id.as_str(), record_missing)?;
        trajectories.push(capture.trajectory_id.as_str().to_string());
    }
    Ok(trajectories)
}

#[cfg(test)]
mod tests;
