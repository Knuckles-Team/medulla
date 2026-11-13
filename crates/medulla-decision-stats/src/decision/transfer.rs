//! The robust transfer estimate across model backbones (MEDULLA-DECISION-ENGINE-R059.1.2).
//!
//! Independent labels of one skill are grouped by the backbone they ran under.
//! Each group is pooled with the same Beta-Binomial kernel the outcome aggregate
//! uses; the robust rate is the lowest pooled rate over the declared backbones.
//! Every declared backbone must report enough labels or the read refuses.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::statistical::log::StoredEvaluation;
use medulla_decision_types::decision::transfer::{
    BackboneRate, SkillCandidateRow, TransferEstimate, TransferRequest, BACKBONE_DUPLICATE,
    BACKBONE_MISSING, TRANSFER_SAMPLES_BELOW_MIN, TRANSFER_SCHEMA_VERSION,
    TRANSFER_TOO_FEW_BACKBONES,
};
use medulla_decision_types::decision::{
    EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef, TraceFidelityLevel,
};
use medulla_numeric::{
    risk::pool_hierarchy, risk::BetaDistribution, risk::ConcentrationBounds, risk::GroupCounts,
    risk::PoolTree,
};
use medulla_types::contract::BoundedVec;

use super::aggregate::{label_use, LabelUse};
use super::quant::q32;
use super::refusal::{Refusal, RefusalResult};

/// One stored evaluation and the principal that decided the record it judges.
#[derive(Debug, Clone, Copy)]
pub struct TransferRow<'a> {
    pub decider: &'a str,
    pub stored: &'a StoredEvaluation,
}

/// The resolved knobs of the transfer read.
#[derive(Debug, Clone, Copy)]
pub struct TransferKnobs {
    pub min_backbones: u64,
    pub min_samples: u64,
    pub fidelity_floor: TraceFidelityLevel,
}

/// One stored evaluation with the question and option of the record it judges.
#[derive(Debug, Clone, Copy)]
pub struct SkillRow<'a> {
    pub question_id: &'a str,
    pub option_id: &'a str,
    pub decider: &'a str,
    pub stored: &'a StoredEvaluation,
}

/// The resolved knobs of the skill-candidate read (MEDULLA-DECISION-ENGINE-R059.2).
#[derive(Debug, Clone, Copy)]
pub struct SkillKnobs {
    pub after_successes: u64,
    pub min_rollouts: u64,
    pub fidelity_floor: TraceFidelityLevel,
}

#[derive(Default)]
struct SkillTally {
    successes: u64,
    records: BTreeSet<String>,
    first_evaluation: Option<String>,
}

/// The options that earned a skill under one question: at least
/// `knobs.after_successes` independent successful labels (the one rule of
/// [`label_use`]) from at least `knobs.min_rollouts` distinct records.
/// Self-reported, claim, below-floor and censored evaluations never count.
/// Rows come out in (question, option) order; the read writes nothing.
pub fn skill_candidates(rows: &[SkillRow], knobs: &SkillKnobs) -> Vec<SkillCandidateRow> {
    let mut tallies: BTreeMap<(&str, &str), SkillTally> = BTreeMap::new();
    for row in rows {
        let LabelUse::Label(true) = label_use(row.stored, row.decider, knobs.fidelity_floor) else {
            continue;
        };
        let tally = tallies.entry((row.question_id, row.option_id)).or_default();
        tally.successes += 1;
        tally
            .records
            .insert(row.stored.evaluation.record_id.clone());
        let evaluation_id = &row.stored.evaluation.evaluation_id;
        if tally
            .first_evaluation
            .as_ref()
            .is_none_or(|first| evaluation_id < first)
        {
            tally.first_evaluation = Some(evaluation_id.clone());
        }
    }
    tallies
        .into_iter()
        .filter(|(_, t)| {
            t.successes >= knobs.after_successes && t.records.len() as u64 >= knobs.min_rollouts
        })
        .map(|((question_id, option_id), t)| SkillCandidateRow {
            question_id: question_id.to_string(),
            option_id: option_id.to_string(),
            successes: t.successes,
            distinct_records: t.records.len() as u64,
            premise: PremiseRef {
                subject: option_id.to_string(),
                fact: format!("transfer.skill_candidate:{option_id}"),
                class: PremiseClass::Observation,
                provenance: PremiseProvenance::Observation {
                    evaluation_id: t.first_evaluation.unwrap_or_default(),
                },
            },
        })
        .collect()
}

#[derive(Default)]
struct Group {
    revision: String,
    weakest: Option<EvidenceClass>,
    trials: u64,
    successes: u64,
}

/// The robust transfer estimate of `request.skill_key` over `rows`.
///
/// Only independent labels (the one rule of [`label_use`]) of bound evaluations
/// of the skill count: self-reported, claim, below-floor and censored
/// evaluations never enter a backbone group.
pub fn robust_transfer(
    rows: &[TransferRow],
    request: &TransferRequest,
    knobs: &TransferKnobs,
) -> RefusalResult<TransferEstimate> {
    let declared: BTreeSet<&str> = request
        .declared_backbones
        .as_slice()
        .iter()
        .map(String::as_str)
        .collect();
    if (declared.len() as u64) < knobs.min_backbones {
        return Err(Refusal::registered(
            TRANSFER_TOO_FEW_BACKBONES,
            format!(
                "{} declared backbones, knob transfer.min_backbones is {}",
                declared.len(),
                knobs.min_backbones
            ),
        ));
    }
    let mut revisions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut groups: BTreeMap<&str, Group> = BTreeMap::new();
    for row in rows {
        let Some(binding) = row.stored.evaluation.backbone.as_ref() else {
            continue;
        };
        if binding.skill_key != request.skill_key {
            continue;
        }
        revisions
            .entry(binding.backbone_id.as_str())
            .or_default()
            .insert(binding.backbone_revision.as_str());
        let LabelUse::Label(success) = label_use(row.stored, row.decider, knobs.fidelity_floor)
        else {
            continue;
        };
        let group = groups.entry(binding.backbone_id.as_str()).or_default();
        group.revision = binding.backbone_revision.clone();
        group.weakest = Some(match (group.weakest, row.stored.evaluation.class) {
            (Some(EvidenceClass::Observation), _) | (_, EvidenceClass::Observation) => {
                EvidenceClass::Observation
            }
            _ => EvidenceClass::Proof,
        });
        group.trials += 1;
        group.successes += u64::from(success);
    }
    for (id, seen) in &revisions {
        if declared.contains(id) && seen.len() > 1 {
            return Err(Refusal::registered(
                BACKBONE_DUPLICATE,
                format!("backbone {id} appears under {} revisions", seen.len()),
            ));
        }
    }
    for id in &declared {
        let Some(group) = groups.get(id) else {
            return Err(Refusal::registered(
                BACKBONE_MISSING,
                format!("declared backbone {id} has no independent labelled row"),
            ));
        };
        if group.trials < knobs.min_samples {
            return Err(Refusal::registered(
                TRANSFER_SAMPLES_BELOW_MIN,
                format!(
                    "backbone {id} has {} labels, knob transfer.min_samples is {}",
                    group.trials, knobs.min_samples
                ),
            ));
        }
    }
    let mut leaves = BTreeMap::new();
    for id in &declared {
        let group = &groups[id];
        leaves.insert(
            (*id).to_string(),
            PoolTree::Leaf(GroupCounts::new(group.successes, group.trials)?),
        );
    }
    let pooled = pool_hierarchy(
        &PoolTree::Branch(leaves),
        BetaDistribution::new(1.0, 1.0)?,
        ConcentrationBounds::new(1.0, 1_000.0)?,
    )?;
    let mut per_backbone = Vec::new();
    let mut lowest = None;
    let mut weakest = EvidenceClass::Proof;
    for id in &declared {
        let group = &groups[id];
        let mean = pooled
            .children
            .get(*id)
            .map(|leaf| leaf.posterior.mean())
            .ok_or_else(|| {
                Refusal::registered(BACKBONE_MISSING, format!("backbone {id} was not pooled"))
            })?;
        let rate = q32(mean)?;
        if group.weakest == Some(EvidenceClass::Observation) {
            weakest = EvidenceClass::Observation;
        }
        // Compare the quantised values, so a tie is exact; ids iterate in order.
        if lowest.is_none_or(
            |(best, _): (medulla_decision_types::decision::QuantisedValue, &str)| {
                rate.value < best.value
            },
        ) {
            lowest = Some((rate, *id));
        }
        per_backbone.push(BackboneRate {
            backbone_id: (*id).to_string(),
            backbone_revision: group.revision.clone(),
            trials: group.trials,
            successes: group.successes,
            rate,
        });
    }
    let (min_rate, min_id) = lowest.ok_or_else(|| {
        Refusal::registered(TRANSFER_TOO_FEW_BACKBONES, "no backbone was declared")
    })?;
    Ok(TransferEstimate {
        schema_version: TRANSFER_SCHEMA_VERSION,
        skill_key: request.skill_key.clone(),
        per_backbone: BoundedVec::new(per_backbone).map_err(|detail| {
            Refusal::new(
                medulla_decision_types::decision::statistical::StatisticalErrorCode::ParameterInvalid,
                detail,
            )
        })?,
        robust_rate: min_rate,
        min_backbone_id: min_id.to_string(),
        evidence_class: weakest,
        premises: BoundedVec::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::dataset::OutcomeFidelity;
    use medulla_decision_types::decision::statistical::log::DecisionOutcomeEvaluation;
    use medulla_decision_types::decision::transfer::BackboneBinding;
    use medulla_decision_types::decision::EvidenceClass;

    const SKILL: &str = "a";

    fn skill() -> String {
        SKILL.repeat(64)
    }

    fn stored(
        backbone: &str,
        revision: &str,
        producer: &str,
        success: bool,
        n: usize,
    ) -> StoredEvaluation {
        StoredEvaluation {
            evaluation: DecisionOutcomeEvaluation {
                record_id: format!("r{n}"),
                evaluation_id: format!("e{n}"),
                class: EvidenceClass::Observation,
                selected_agent: "agent".to_string(),
                lease_holder: "lease".to_string(),
                fidelity: OutcomeFidelity::FullStep,
                success: Some(success),
                backbone: Some(BackboneBinding {
                    backbone_id: backbone.to_string(),
                    backbone_revision: revision.to_string(),
                    skill_key: skill(),
                    task_distribution_id: "tasks".to_string(),
                    task_distribution_revision: "1".to_string(),
                    dataset_digest: "b".repeat(64),
                    metric_id: "success".to_string(),
                    metric_revision: "1".to_string(),
                }),
            },
            producer: producer.to_string(),
            recorded_at_ms: 0,
        }
    }

    fn batch(
        backbone: &str,
        revision: &str,
        successes: usize,
        total: usize,
    ) -> Vec<StoredEvaluation> {
        (0..total)
            .map(|n| stored(backbone, revision, "judge", n < successes, n))
            .collect()
    }

    fn request(ids: &[&str]) -> TransferRequest {
        TransferRequest {
            skill_key: skill(),
            declared_backbones: BoundedVec::new(ids.iter().map(|s| s.to_string()).collect())
                .unwrap(),
        }
    }

    fn knobs() -> TransferKnobs {
        TransferKnobs {
            min_backbones: 2,
            min_samples: 30,
            fidelity_floor: TraceFidelityLevel::FullStep,
        }
    }

    fn run(
        evals: &[StoredEvaluation],
        ids: &[&str],
        knobs: &TransferKnobs,
    ) -> RefusalResult<TransferEstimate> {
        let rows: Vec<TransferRow> = evals
            .iter()
            .map(|stored| TransferRow {
                decider: "decider",
                stored,
            })
            .collect();
        robust_transfer(&rows, &request(ids), knobs)
    }

    // spec: MEDULLA-DECISION-ENGINE-R059.1.2
    #[test]
    fn robust_rate_is_the_backbone_minimum_and_missing_backbones_refuse() {
        let mut evals = batch("high", "1", 36, 40);
        evals.extend(batch("low", "1", 20, 40));
        let estimate = run(&evals, &["high", "low"], &knobs()).expect("estimate");
        assert_eq!(estimate.per_backbone.as_slice().len(), 2);
        assert_eq!(estimate.min_backbone_id, "low");
        let low = estimate
            .per_backbone
            .iter()
            .find(|row| row.backbone_id == "low")
            .unwrap();
        let high = estimate
            .per_backbone
            .iter()
            .find(|row| row.backbone_id == "high")
            .unwrap();
        assert!(low.rate.value < high.rate.value);
        assert_eq!(estimate.robust_rate, low.rate);
        assert_eq!((low.trials, low.successes), (40, 20));

        // A tie names the lowest backbone id.
        let mut tie = batch("b-two", "1", 20, 40);
        tie.extend(batch("a-one", "1", 20, 40));
        let tied = run(&tie, &["b-two", "a-one"], &knobs()).unwrap();
        assert_eq!(tied.min_backbone_id, "a-one");

        // A declared backbone without rows refuses, naming it.
        let missing = run(&batch("high", "1", 36, 40), &["high", "ghost"], &knobs()).unwrap_err();
        assert_eq!(missing.code, "BACKBONE_MISSING");
        assert!(missing.detail.contains("ghost"));

        // Self-reported evaluations never enter a group: only self-reports
        // for "low" leaves it without rows.
        let mut selfish = batch("high", "1", 36, 40);
        selfish.extend((0..40).map(|n| stored("low", "1", "agent", true, n)));
        let refused = run(&selfish, &["high", "low"], &knobs()).unwrap_err();
        assert_eq!(refused.code, "BACKBONE_MISSING");

        // One backbone id under two revisions.
        let mut dup = batch("high", "1", 36, 40);
        dup.extend(batch("low", "1", 20, 40));
        dup.extend((40..50).map(|n| stored("low", "2", "judge", true, n)));
        assert_eq!(
            run(&dup, &["high", "low"], &knobs()).unwrap_err().code,
            "BACKBONE_DUPLICATE"
        );

        // Too few declared backbones; too few labels.
        assert_eq!(
            run(&evals, &["high"], &knobs()).unwrap_err().code,
            "TRANSFER_TOO_FEW_BACKBONES"
        );
        let thin = TransferKnobs {
            min_samples: 41,
            ..knobs()
        };
        assert_eq!(
            run(&evals, &["high", "low"], &thin).unwrap_err().code,
            "TRANSFER_SAMPLES_BELOW_MIN"
        );
    }

    fn skill_knobs() -> SkillKnobs {
        SkillKnobs {
            after_successes: 3,
            min_rollouts: 3,
            fidelity_floor: TraceFidelityLevel::FullStep,
        }
    }

    fn skill_rows<'a>(
        evals: &'a [(&'a str, &'a StoredEvaluation)],
        question: &'a str,
        option: &'a str,
    ) -> Vec<SkillRow<'a>> {
        evals
            .iter()
            .map(|(decider, stored)| SkillRow {
                question_id: question,
                option_id: option,
                decider,
                stored,
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R059.2
    #[test]
    fn skill_needs_successes_from_enough_distinct_records() {
        // Three successes on three records qualify; each question is its own scope.
        let evals: Vec<StoredEvaluation> =
            (0..3).map(|n| stored("b", "1", "judge", true, n)).collect();
        let pairs: Vec<(&str, &StoredEvaluation)> = evals.iter().map(|e| ("decider", e)).collect();
        let rows = skill_candidates(&skill_rows(&pairs, "q", "opt"), &skill_knobs());
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].successes, rows[0].distinct_records), (3, 3));
        assert_eq!(rows[0].premise.fact, "transfer.skill_candidate:opt");

        // Two successes, or one failure among three, do not qualify.
        assert!(skill_candidates(&skill_rows(&pairs[..2], "q", "opt"), &skill_knobs()).is_empty());
        let mixed = [
            stored("b", "1", "judge", true, 0),
            stored("b", "1", "judge", true, 1),
            stored("b", "1", "judge", false, 2),
        ];
        let pairs: Vec<(&str, &StoredEvaluation)> = mixed.iter().map(|e| ("decider", e)).collect();
        assert!(skill_candidates(&skill_rows(&pairs, "q", "opt"), &skill_knobs()).is_empty());

        // Three successes of one record fail the distinct-record floor.
        let mut same = vec![
            stored("b", "1", "judge-a", true, 0),
            stored("b", "1", "judge-b", true, 0),
            stored("b", "1", "judge-c", true, 0),
        ];
        for (i, e) in same.iter_mut().enumerate() {
            e.evaluation.evaluation_id = format!("e-{i}");
        }
        let pairs: Vec<(&str, &StoredEvaluation)> = same.iter().map(|e| ("decider", e)).collect();
        assert!(skill_candidates(&skill_rows(&pairs, "q", "opt"), &skill_knobs()).is_empty());

        // Self-reported successes never count.
        let selfish: Vec<StoredEvaluation> = (0..3)
            .map(|n| stored("b", "1", "decider", true, n))
            .collect();
        let pairs: Vec<(&str, &StoredEvaluation)> =
            selfish.iter().map(|e| ("decider", e)).collect();
        assert!(skill_candidates(&skill_rows(&pairs, "q", "opt"), &skill_knobs()).is_empty());
    }
}
