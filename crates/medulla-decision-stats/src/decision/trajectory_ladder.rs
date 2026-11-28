//! The width and depth rung kernel and the paired-verdict check
//! (MEDULLA-DECISION-ENGINE-R134.12, R134.12.1).
//!
//! Pure and deterministic: no clock, no store. A rung is legal when its width
//! is within the permitted bound and its depth within `trajectory.max_depth`.
//! Legal rungs are ordered by `width * depth`, then width, then depth, then rung
//! id. Without a promoted head the knob index picks the rung; with one, the
//! head's score does, over the legal set only, so a head never widens it.

use std::cmp::Ordering;

use medulla_decision_types::decision::statistical::head::DecisionHeadBody;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_policy_types::decision::statistical::dataset::ItemLabel,
    medulla_decision_policy_types::decision::statistical::dataset::LabelSource,
    medulla_decision_policy_types::decision::statistical::dataset::LabelledItem,
};
use {
    medulla_decision_program_types::decision::trajectory_ladder::decision_digest_prefix,
    medulla_decision_program_types::decision::trajectory_ladder::PairedVerdict,
    medulla_decision_program_types::decision::trajectory_ladder::RungChoice,
    medulla_decision_program_types::decision::trajectory_ladder::TrajectoryLadderRequest,
    medulla_decision_program_types::decision::trajectory_ladder::TrajectoryRung,
    medulla_decision_program_types::decision::trajectory_ladder::Winner,
    medulla_decision_program_types::decision::trajectory_ladder::TRAJECTORY_ARM_MISSING,
    medulla_decision_program_types::decision::trajectory_ladder::TRAJECTORY_BASELINE_MISSING,
    medulla_decision_program_types::decision::trajectory_ladder::TRAJECTORY_NO_LEGAL_RUNG,
    medulla_decision_program_types::decision::trajectory_ladder::TRAJECTORY_SNAPSHOT_MISMATCH,
};

/// Why the ladder cannot choose a rung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderRefusal {
    /// The rung set lacks the baseline rung.
    BaselineMissing,
    /// No rung is legal under the width and depth bounds.
    NoLegalRung,
    /// The promoted head does not score a (width, depth) rung.
    HeadShape(String),
}

impl LadderRefusal {
    /// The refusal as the typed `CODE: detail` text, for the codes the family registers.
    pub fn render(&self) -> Option<String> {
        match self {
            Self::BaselineMissing => Some(format!(
                "{TRAJECTORY_BASELINE_MISSING}: the rung set holds no rung of width 1 and depth 1"
            )),
            Self::NoLegalRung => Some(format!(
                "{TRAJECTORY_NO_LEGAL_RUNG}: no rung is within the width and depth bounds"
            )),
            Self::HeadShape(_) => None,
        }
    }
}

fn order(a: &TrajectoryRung, b: &TrajectoryRung) -> Ordering {
    (
        u64::from(a.width) * u64::from(a.depth),
        a.width,
        a.depth,
        &a.rung_id,
    )
        .cmp(&(
            u64::from(b.width) * u64::from(b.depth),
            b.width,
            b.depth,
            &b.rung_id,
        ))
}

/// The score a head gives one rung: the weighted, standardised `[width, depth]`
/// features in Q32. The head must carry exactly those two weights.
pub fn head_score(head: &DecisionHeadBody, rung: &TrajectoryRung) -> Result<i128, LadderRefusal> {
    if head.weights.len() != 2 || head.standardisation.len() != 2 {
        return Err(LadderRefusal::HeadShape(
            "a ladder head scores exactly two features, width and depth".to_string(),
        ));
    }
    let features = [i128::from(rung.width), i128::from(rung.depth)];
    let mut score: i128 = 0;
    for (index, feature) in features.iter().enumerate() {
        let stand = &head.standardisation.as_slice()[index];
        let weight = i128::from(head.weights.as_slice()[index].value);
        let scale = i128::from(stand.scale.value).max(1);
        let z = (((feature << 32) - i128::from(stand.center.value)) << 32) / scale;
        let z = z.clamp(
            i128::from(stand.lower.value),
            i128::from(stand.upper.value).max(i128::from(stand.lower.value)),
        );
        score += (weight * z) >> 32;
    }
    Ok(score)
}

/// Choose the rung (R134.12). `head` is the promoted head the caller pinned.
pub fn choose_rung(
    request: &TrajectoryLadderRequest,
    max_depth: u32,
    default_rung: usize,
    head: Option<&DecisionHeadBody>,
) -> Result<RungChoice, LadderRefusal> {
    let baseline = request
        .rungs
        .iter()
        .find(|rung| rung.is_baseline())
        .ok_or(LadderRefusal::BaselineMissing)?;
    let mut legal: Vec<&TrajectoryRung> = request
        .rungs
        .iter()
        .filter(|rung| {
            rung.width >= 1
                && rung.depth >= 1
                && rung.width <= request.max_width
                && rung.depth <= max_depth
        })
        .collect();
    legal.sort_by(|a, b| order(a, b));
    if legal.is_empty() {
        return Err(LadderRefusal::NoLegalRung);
    }
    let chosen = match head {
        Some(head) => {
            let mut best = legal[0];
            let mut best_score = head_score(head, best)?;
            for rung in &legal[1..] {
                let score = head_score(head, rung)?;
                if score > best_score {
                    best = rung;
                    best_score = score;
                }
            }
            best
        }
        None => legal[default_rung.min(legal.len() - 1)],
    };
    let prefix = decision_digest_prefix(request, &chosen.rung_id);
    Ok(RungChoice {
        rung_id: chosen.rung_id.clone(),
        width: chosen.width,
        depth: chosen.depth,
        baseline_id: baseline.rung_id.clone(),
        trajectory_ids: (1..=chosen.width)
            .map(|k| format!("{prefix}-t{k}"))
            .collect(),
        ranked_by_head: head.is_some(),
    })
}

fn is_digest(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Check a verdict before it is stored (R134.12.1): both arms carry an outcome
/// digest and the snapshot is the one the decision was asked on.
pub fn check_paired_verdict(
    verdict: &PairedVerdict,
    decision_input_snapshot_digest: &str,
) -> Result<(), String> {
    if !is_digest(&verdict.arm_outcome_digest) || !is_digest(&verdict.baseline_outcome_digest) {
        return Err(format!(
            "{TRAJECTORY_ARM_MISSING}: both arms carry an outcome digest"
        ));
    }
    if verdict.input_snapshot_digest != decision_input_snapshot_digest {
        return Err(format!(
            "{TRAJECTORY_SNAPSHOT_MISMATCH}: the verdict's snapshot is not the decision's input snapshot"
        ));
    }
    Ok(())
}

/// Whether a stored verdict proves `rung_id` improved on the baseline on this
/// snapshot: its winner is the arm.
pub fn improvement_proven(
    verdicts: &[PairedVerdict],
    rung_id: &str,
    input_snapshot_digest: &str,
) -> bool {
    verdicts.iter().any(|verdict| {
        verdict.rung_id == rung_id
            && verdict.input_snapshot_digest == input_snapshot_digest
            && verdict.winner == Winner::Arm
    })
}

/// The feature columns of the ladder head's dataset, in the order `head_score`
/// reads them.
pub const LADDER_FEATURE_NAMES: [&str; 2] = ["width", "depth"];

/// The class key of every verdict item: one calibration class.
pub const VERDICT_CLASS_KEY: &str = "trajectory_ladder";

/// One stored verdict joined with the two rungs it compares and the time the
/// decision it answers was made.
pub struct VerdictRow<'a> {
    pub verdict: &'a PairedVerdict,
    pub arm: &'a TrajectoryRung,
    pub baseline: &'a TrajectoryRung,
    pub decided_at_ms: u64,
}

/// The labelled item of one stored verdict (R134.12.1): the candidates are the
/// arm rung and the baseline rung, their features are `[width, depth]` in Q32,
/// and the acceptable candidates are the winner (both on a tie). Returns `None`
/// when the verdict does not name the baseline it was compared against.
pub fn verdict_item(row: &VerdictRow<'_>) -> Option<LabelledItem> {
    if row.verdict.rung_id != row.arm.rung_id || row.verdict.baseline_id != row.baseline.rung_id {
        return None;
    }
    let acceptable = match row.verdict.winner {
        Winner::Arm => vec![row.arm.rung_id.clone()],
        Winner::Baseline => vec![row.baseline.rung_id.clone()],
        Winner::Tie => vec![row.arm.rung_id.clone(), row.baseline.rung_id.clone()],
    };
    let q32 = |value: u32| i64::from(value) << 32;
    Some(LabelledItem {
        item_id: format!("{}/{}", row.verdict.decision_id, row.verdict.rung_id),
        recorded_at_ms: row.decided_at_ms,
        class_key: VERDICT_CLASS_KEY.to_string(),
        candidate_ids: BoundedVec::new(vec![row.arm.rung_id.clone(), row.baseline.rung_id.clone()])
            .ok()?,
        features: BoundedVec::new(vec![
            q32(row.arm.width),
            q32(row.arm.depth),
            q32(row.baseline.width),
            q32(row.baseline.depth),
        ])
        .ok()?,
        label: ItemLabel::Gold {
            acceptable: BoundedVec::new(acceptable).ok()?,
            source: LabelSource::SyntheticConstruction,
        },
        audit_inclusion: None,
    })
}
