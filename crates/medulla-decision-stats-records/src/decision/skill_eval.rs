//! Pure skill-evaluation kernels (MEDULLA-DECISION-ENGINE-R711.2, R711.4).
//!
//! No I/O and no clock. [`trajectory_score`] and the outcome score are separate numbers that
//! never merge; [`suite_mix`] and [`pass_band`] describe a suite; the checks refuse a judged
//! case under an uncalibrated judge, a result of a run that read the host, and a case the kind or
//! its mocks make unrunnable. All shares are integer parts per million.

use std::collections::BTreeSet;

use medulla_decision_types::correction::{CorrectionSource, EvalCase};
use medulla_decision_types::decision::codes::{
    ABLATION_NOISE_BAND_MISSING, CANARY_TRAFFIC_OUT_OF_BOUNDS,
};
use medulla_decision_types::knowledge_flag::target_form;
use medulla_decision_types::skill_eval::{
    AblationVerdict, CanaryVerdict, CompileState, EnvironmentAttestation, EvalCaseKind,
    EvalCaseSource, JudgeCalibration, ModelCompile, NoiseBand, SkillEvalCase, SuiteBand,
    TrajectoryStep, EVAL_CASE_INVALID, EVAL_RUN_NOT_CLEAN_ROOM, JUDGE_MIN_ANNOTATED,
    JUDGE_NOT_CALIBRATED,
};
use medulla_types::refusal_code::RefusalCode;

const PPM: u64 = 1_000_000;

/// The knobs of a suite, read once by the family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvalKnobs {
    /// Pass rate below which a suite is too low.
    pub low_ppm: u64,
    /// Pass rate above which a suite is saturated.
    pub high_ppm: u64,
    /// Wanted share of golden cases.
    pub golden_ppm: u64,
    /// Wanted share of real-prompt cases.
    pub real_ppm: u64,
    /// Wanted share of author-judged cases.
    pub judged_ppm: u64,
    /// How far a share may sit from its wanted share.
    pub tolerance_ppm: u64,
    /// Fewest cases a suite needs.
    pub min_cases: u64,
    /// Lowest judge kappa that lets a judged case count.
    pub judge_min_kappa_ppm: u64,
}

/// Why a check refused: one registered code and a short reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalRefusal {
    /// The registered code.
    pub code: RefusalCode,
    /// What was wrong.
    pub reason: String,
}

fn refuse(code: RefusalCode, reason: impl Into<String>) -> EvalRefusal {
    EvalRefusal {
        code,
        reason: reason.into(),
    }
}

/// The longest common subsequence of `(kind, name)` pairs over the longer length, in ppm. Two
/// empty trajectories match fully.
pub fn trajectory_score(expected: &[TrajectoryStep], actual: &[TrajectoryStep]) -> u64 {
    let longer = expected.len().max(actual.len());
    if longer == 0 {
        return PPM;
    }
    let mut table = vec![vec![0u64; actual.len() + 1]; expected.len() + 1];
    for (i, want) in expected.iter().enumerate() {
        for (j, got) in actual.iter().enumerate() {
            table[i + 1][j + 1] = if want == got {
                table[i][j] + 1
            } else {
                table[i][j + 1].max(table[i + 1][j])
            };
        }
    }
    table[expected.len()][actual.len()] * PPM / longer as u64
}

/// The outcome score of one case: full when the answer digest is the expected one.
pub fn outcome_score(expected_digest: Option<&str>, actual_digest: &str) -> u64 {
    if expected_digest == Some(actual_digest) {
        PPM
    } else {
        0
    }
}

/// The golden, real-prompt and author-judged shares of `cases`, in ppm.
pub fn suite_mix(cases: &[SkillEvalCase]) -> (u64, u64, u64) {
    if cases.is_empty() {
        return (0, 0, 0);
    }
    let share = |kind: EvalCaseKind| {
        cases.iter().filter(|case| case.kind == kind).count() as u64 * PPM / cases.len() as u64
    };
    (
        share(EvalCaseKind::Golden),
        share(EvalCaseKind::RealPrompt),
        share(EvalCaseKind::AuthorJudged),
    )
}

/// Whether the mix sits within the tolerance of every wanted share.
pub fn mix_in_band(mix: (u64, u64, u64), knobs: &EvalKnobs) -> bool {
    let near = |share: u64, wanted: u64| share.abs_diff(wanted) <= knobs.tolerance_ppm;
    near(mix.0, knobs.golden_ppm) && near(mix.1, knobs.real_ppm) && near(mix.2, knobs.judged_ppm)
}

/// The band of a pass rate over `n` cases. A suite under `eval.min_cases` is `TooLow`.
pub fn pass_band(pass_rate_ppm: u64, n: u64, knobs: &EvalKnobs) -> SuiteBand {
    if n < knobs.min_cases || pass_rate_ppm < knobs.low_ppm {
        SuiteBand::TooLow
    } else if pass_rate_ppm > knobs.high_ppm {
        SuiteBand::Saturated
    } else {
        SuiteBand::Healthy
    }
}

/// A judged case needs a judge whose kappa reaches `eval.judge.min_kappa_ppm` over at least 50
/// annotated items.
pub fn check_judge(
    case: &SkillEvalCase,
    calibration: Option<&JudgeCalibration>,
    knobs: &EvalKnobs,
) -> Result<(), EvalRefusal> {
    if case.kind != EvalCaseKind::AuthorJudged {
        return Ok(());
    }
    match calibration {
        Some(judge)
            if judge.kappa_ppm >= knobs.judge_min_kappa_ppm && judge.n >= JUDGE_MIN_ANNOTATED =>
        {
            Ok(())
        }
        Some(judge) => Err(refuse(
            JUDGE_NOT_CALIBRATED,
            format!(
                "judge {} has kappa {} over {} items (needs {} over {})",
                judge.judge_ref,
                judge.kappa_ppm,
                judge.n,
                knobs.judge_min_kappa_ppm,
                JUDGE_MIN_ANNOTATED
            ),
        )),
        None => Err(refuse(
            JUDGE_NOT_CALIBRATED,
            "the case names no calibrated judge",
        )),
    }
}

/// The `SkillEvalCase` a correction of a skill yields (MEDULLA-DECISION-ENGINE-R702.7): the
/// `EvalCase` effect of the correction becomes a golden case, with source `Correction` for a human
/// correction and `ReviewSignal` for a review comment or rejection. A correction whose target is
/// not `skill:<name>@<revision>` yields none. Only digests are carried, never text.
pub fn case_from_correction(
    target: &str,
    source: CorrectionSource,
    eval_case: &EvalCase,
) -> Option<SkillEvalCase> {
    match target_form(target) {
        Some(("skill:", _)) => {}
        _ => return None,
    }
    Some(SkillEvalCase {
        case_id: format!("skill-{}", eval_case.case_id),
        skill_ref: target.to_string(),
        kind: EvalCaseKind::Golden,
        input_digest: eval_case.input_digest.clone(),
        expected_digest: Some(eval_case.expected_digest.clone()),
        tool_mocks: Vec::new(),
        expected_trajectory: Vec::new(),
        source: match source {
            CorrectionSource::Human => EvalCaseSource::Correction,
            CorrectionSource::ReviewComment | CorrectionSource::ReviewRejection => {
                EvalCaseSource::ReviewSignal
            }
        },
    })
}

/// A result of a run that read the host is not stored.
pub fn check_clean_room(attestation: &EnvironmentAttestation) -> Result<(), EvalRefusal> {
    if attestation.host_paths_read > 0 {
        return Err(refuse(
            EVAL_RUN_NOT_CLEAN_ROOM,
            format!("the run read {} host paths", attestation.host_paths_read),
        ));
    }
    Ok(())
}

/// A golden case needs an expected digest, a real-prompt case an expected trajectory, and every
/// mock names a tool the skill may call.
pub fn check_case(case: &SkillEvalCase, known_tools: &BTreeSet<String>) -> Result<(), EvalRefusal> {
    if case.kind == EvalCaseKind::Golden && case.expected_digest.is_none() {
        return Err(refuse(
            EVAL_CASE_INVALID,
            "a golden case names no expected_digest",
        ));
    }
    if case.kind == EvalCaseKind::RealPrompt && case.expected_trajectory.is_empty() {
        return Err(refuse(
            EVAL_CASE_INVALID,
            "a real-prompt case names no expected_trajectory",
        ));
    }
    if let Some(mock) = case
        .tool_mocks
        .iter()
        .find(|mock| !known_tools.contains(&mock.tool))
    {
        return Err(refuse(
            EVAL_CASE_INVALID,
            format!("the case mocks unknown tool {}", mock.tool),
        ));
    }
    Ok(())
}

/// The noise band of identical runs: twice their standard deviation, in ppm
/// (MEDULLA-DECISION-ENGINE-R712.2). `min_runs` is knob `eval.ablation.identical_runs`; fewer
/// scores refuse `ABLATION_NOISE_BAND_MISSING`. Integer arithmetic only, rounded down.
pub fn noise_band(identical_scores_ppm: &[u64], min_runs: u64) -> Result<NoiseBand, EvalRefusal> {
    let n = identical_scores_ppm.len() as u64;
    if n < min_runs.max(2) {
        return Err(refuse(
            ABLATION_NOISE_BAND_MISSING,
            format!("{n} identical runs are fewer than the {min_runs} a noise band needs"),
        ));
    }
    let sum: u128 = identical_scores_ppm
        .iter()
        .map(|score| u128::from(*score))
        .sum();
    let squares: u128 = identical_scores_ppm
        .iter()
        .map(|score| u128::from(*score) * u128::from(*score))
        .sum();
    // n^2 times the variance: n * sum(x^2) - (sum x)^2, never negative.
    let spread = u128::from(n) * squares - sum * sum;
    let band = 2 * spread.isqrt() / u128::from(n);
    Ok(NoiseBand {
        identical_runs: n,
        band_ppm: u64::try_from(band).unwrap_or(u64::MAX),
    })
}

fn mean_ppm(scores: &[u64]) -> i64 {
    if scores.is_empty() {
        return 0;
    }
    let sum: i128 = scores.iter().map(|score| i128::from(*score)).sum();
    i64::try_from(sum.div_euclid(scores.len() as i128)).unwrap_or(i64::MAX)
}

/// With against without: `Effect` only when the absolute difference of the arm means exceeds the
/// noise band; the delta is `mean(with) - mean(without)` (MEDULLA-DECISION-ENGINE-R712.2). An arm
/// with no score is no evidence of an effect.
pub fn ablation_verdict(with: &[u64], without: &[u64], band_ppm: u64) -> AblationVerdict {
    if with.is_empty() || without.is_empty() {
        return AblationVerdict::WithinNoise;
    }
    let delta_ppm = mean_ppm(with) - mean_ppm(without);
    if delta_ppm.unsigned_abs() > band_ppm {
        AblationVerdict::Effect { delta_ppm }
    } else {
        AblationVerdict::WithinNoise
    }
}

/// The canary bucket of one run: the first 8 bytes of the framed SHA-256 of the run id and the
/// skill, as a big-endian number modulo one million (MEDULLA-DECISION-ENGINE-R712.2).
pub fn canary_bucket(run_id: &str, skill_ref: &str) -> u64 {
    let digest = medulla_types::contract::Digest256::framed(
        b"medulla.skill.canary.v1",
        &[run_id.as_bytes(), skill_ref.as_bytes()],
    )
    .expect("a canary bucket frames two short fields");
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_be_bytes(head) % PPM
}

/// Whether a run takes the challenger: its bucket is below the canary traffic share.
pub fn takes_challenger(bucket: u64, traffic_ppm: u64) -> bool {
    bucket < traffic_ppm
}

/// A traffic share inside the bounds of knob `canary.traffic_ppm`, or the refusal
/// (MEDULLA-DECISION-ENGINE-R712.4).
pub fn check_traffic(traffic_ppm: u64, low: u64, high: u64) -> Result<u64, EvalRefusal> {
    if (low..=high).contains(&traffic_ppm) {
        Ok(traffic_ppm)
    } else {
        Err(refuse(
            CANARY_TRAFFIC_OUT_OF_BOUNDS,
            format!("traffic {traffic_ppm} is outside [{low}, {high}]"),
        ))
    }
}

/// What a canary decides (MEDULLA-DECISION-ENGINE-R712.3): a regression verdict rolls back; a
/// challenger whose paired lower bound clears the noise band ramps; anything else holds.
pub fn canary_verdict(delta_lower_ppm: i64, band_ppm: u64, regressed: bool) -> CanaryVerdict {
    if regressed {
        CanaryVerdict::Rollback
    } else if delta_lower_ppm > 0 && delta_lower_ppm.unsigned_abs() > band_ppm {
        CanaryVerdict::Ramp
    } else {
        CanaryVerdict::Hold
    }
}

/// The rows of `model_ref` that a change of its model revision makes stale: every `Current` row
/// of that model but the one just compiled (MEDULLA-DECISION-ENGINE-R712.2).
pub fn stale_after_change(
    rows: &[ModelCompile],
    model_ref: &str,
    keep_skill: &str,
) -> Vec<ModelCompile> {
    rows.iter()
        .filter(|row| {
            row.model_ref == model_ref
                && row.state == CompileState::Current
                && row.skill_ref != keep_skill
        })
        .map(|row| ModelCompile {
            state: CompileState::Stale,
            ..row.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::skill_eval::{EvalCaseSource, ToolMock, TrajectoryKind};

    const KNOBS: EvalKnobs = EvalKnobs {
        low_ppm: 700_000,
        high_ppm: 900_000,
        golden_ppm: 50_000,
        real_ppm: 800_000,
        judged_ppm: 150_000,
        tolerance_ppm: 100_000,
        min_cases: 20,
        judge_min_kappa_ppm: 600_000,
    };

    fn step(kind: TrajectoryKind, name: &str) -> TrajectoryStep {
        TrajectoryStep {
            kind,
            name: name.into(),
        }
    }

    fn case(kind: EvalCaseKind) -> SkillEvalCase {
        SkillEvalCase {
            case_id: "c".into(),
            skill_ref: "skill:triage@3".into(),
            kind,
            input_digest: "i".into(),
            expected_digest: Some("e".into()),
            tool_mocks: Vec::new(),
            expected_trajectory: vec![step(TrajectoryKind::Tool, "search")],
            source: EvalCaseSource::Authored,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R711.2
    #[test]
    fn trajectory_and_outcome_score_apart() {
        let want = [
            step(TrajectoryKind::Skill, "triage"),
            step(TrajectoryKind::Tool, "search"),
            step(TrajectoryKind::Tool, "reply"),
        ];
        let got = [
            step(TrajectoryKind::Skill, "triage"),
            step(TrajectoryKind::Tool, "reply"),
        ];
        // The common subsequence is two of the longer three.
        assert_eq!(trajectory_score(&want, &got), 666_666);
        assert_eq!(trajectory_score(&want, &want), 1_000_000);
        assert_eq!(trajectory_score(&[], &[]), 1_000_000);
        // A right answer by a wrong path keeps the two scores apart.
        assert_eq!(outcome_score(Some("e"), "e"), 1_000_000);
        assert_eq!(outcome_score(Some("e"), "x"), 0);
        assert_eq!(outcome_score(None, "x"), 0);
        // Same kind and name only: a tool named like a skill is a different step.
        assert_eq!(
            trajectory_score(
                &[step(TrajectoryKind::Skill, "a")],
                &[step(TrajectoryKind::Tool, "a")]
            ),
            0
        );

        let cases: Vec<SkillEvalCase> = std::iter::once(case(EvalCaseKind::Golden))
            .chain(std::iter::repeat_with(|| case(EvalCaseKind::RealPrompt)).take(16))
            .chain(std::iter::repeat_with(|| case(EvalCaseKind::AuthorJudged)).take(3))
            .collect();
        let mix = suite_mix(&cases);
        assert_eq!(mix, (50_000, 800_000, 150_000));
        assert!(mix_in_band(mix, &KNOBS));
        assert!(!mix_in_band((500_000, 500_000, 0), &KNOBS));

        assert_eq!(pass_band(800_000, 20, &KNOBS), SuiteBand::Healthy);
        assert_eq!(pass_band(950_000, 20, &KNOBS), SuiteBand::Saturated);
        assert_eq!(pass_band(500_000, 20, &KNOBS), SuiteBand::TooLow);
        assert_eq!(pass_band(800_000, 19, &KNOBS), SuiteBand::TooLow);
    }

    // spec: MEDULLA-DECISION-ENGINE-R702.7
    #[test]
    fn corrections_of_skills_yield_cases_by_source() {
        let effect = EvalCase {
            case_id: "case:r1".into(),
            input_digest: "in".into(),
            expected_digest: "out".into(),
        };
        let human = case_from_correction("skill:triage@3", CorrectionSource::Human, &effect)
            .expect("a skill target yields a case");
        assert_eq!(human.source, EvalCaseSource::Correction);
        assert_eq!(human.kind, EvalCaseKind::Golden);
        assert_eq!(human.expected_digest.as_deref(), Some("out"));
        let review =
            case_from_correction("skill:triage@3", CorrectionSource::ReviewRejection, &effect)
                .expect("a review yields a case");
        assert_eq!(review.source, EvalCaseSource::ReviewSignal);
        assert!(check_case(&human, &BTreeSet::new()).is_ok());
        assert!(case_from_correction("answer:9", CorrectionSource::Human, &effect).is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R711.4
    #[test]
    fn checks_refuse_with_registered_codes() {
        let judge = |kappa, n| JudgeCalibration {
            judge_ref: "j".into(),
            annotated_set_ref: "s".into(),
            kappa_ppm: kappa,
            n,
        };
        let judged = case(EvalCaseKind::AuthorJudged);
        assert!(check_judge(&judged, Some(&judge(700_000, 50)), &KNOBS).is_ok());
        for bad in [Some(judge(400_000, 50)), Some(judge(700_000, 49)), None] {
            assert_eq!(
                check_judge(&judged, bad.as_ref(), &KNOBS).unwrap_err().code,
                JUDGE_NOT_CALIBRATED
            );
        }
        assert!(check_judge(&case(EvalCaseKind::Golden), None, &KNOBS).is_ok());

        let attestation = |read| EnvironmentAttestation {
            sandbox_digest: "s".into(),
            env_digest: "e".into(),
            host_paths_read: read,
        };
        assert!(check_clean_room(&attestation(0)).is_ok());
        assert_eq!(
            check_clean_room(&attestation(1)).unwrap_err().code,
            EVAL_RUN_NOT_CLEAN_ROOM
        );

        let tools: BTreeSet<String> = ["search".to_string()].into();
        let mut golden = case(EvalCaseKind::Golden);
        golden.expected_digest = None;
        assert_eq!(
            check_case(&golden, &tools).unwrap_err().code,
            EVAL_CASE_INVALID
        );
        let mut real = case(EvalCaseKind::RealPrompt);
        real.expected_trajectory.clear();
        assert_eq!(
            check_case(&real, &tools).unwrap_err().code,
            EVAL_CASE_INVALID
        );
        let mut mocked = case(EvalCaseKind::RealPrompt);
        mocked.tool_mocks = vec![ToolMock {
            tool: "delete".into(),
            args_digest: "a".into(),
            result_digest: "r".into(),
        }];
        assert_eq!(
            check_case(&mocked, &tools).unwrap_err().code,
            EVAL_CASE_INVALID
        );
        assert!(check_case(&case(EvalCaseKind::RealPrompt), &tools).is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R712.2
    #[test]
    fn noise_band_and_canary_bucket_are_deterministic() {
        // Twice the standard deviation of five identical runs, rounded down.
        let band = noise_band(&[100, 200, 300, 400, 500], 5).expect("five runs");
        assert_eq!(
            band,
            NoiseBand {
                identical_runs: 5,
                band_ppm: 282
            }
        );
        // Fewer runs than the knob refuse; identical scores have no noise.
        assert_eq!(
            noise_band(&[1, 2, 3, 4], 5).unwrap_err().code,
            ABLATION_NOISE_BAND_MISSING
        );
        assert_eq!(noise_band(&[7, 7, 7, 7, 7], 5).unwrap().band_ppm, 0);

        // The verdict compares the mean difference with the band.
        let with = [700_000, 710_000, 690_000];
        let without = [600_000, 610_000, 590_000];
        assert_eq!(
            ablation_verdict(&with, &without, 282),
            AblationVerdict::Effect { delta_ppm: 100_000 }
        );
        assert_eq!(
            ablation_verdict(&with, &without, 100_000),
            AblationVerdict::WithinNoise
        );
        assert_eq!(
            ablation_verdict(&without, &with, 282),
            AblationVerdict::Effect {
                delta_ppm: -100_000
            }
        );
        assert_eq!(
            ablation_verdict(&[], &without, 0),
            AblationVerdict::WithinNoise
        );

        // A bucket is a pure function of the run and the skill, below one million.
        let bucket = canary_bucket("run-1", "skill:triage@3");
        assert_eq!(bucket, canary_bucket("run-1", "skill:triage@3"));
        assert!(bucket < 1_000_000);
        assert_ne!(bucket, canary_bucket("run-2", "skill:triage@3"));
        assert!(takes_challenger(49_999, 50_000) && !takes_challenger(50_000, 50_000));
        let taking = (0..2_000)
            .filter(|index| takes_challenger(canary_bucket(&format!("run-{index}"), "s"), 100_000))
            .count();
        assert!(
            (100..300).contains(&taking),
            "about a tenth of 2000 runs, got {taking}"
        );

        assert_eq!(check_traffic(50_000, 1_000, 500_000), Ok(50_000));
        assert_eq!(
            check_traffic(900, 1_000, 500_000).unwrap_err().code,
            CANARY_TRAFFIC_OUT_OF_BOUNDS
        );
        assert_eq!(canary_verdict(5_000, 1_000, false), CanaryVerdict::Ramp);
        assert_eq!(canary_verdict(500, 1_000, false), CanaryVerdict::Hold);
        assert_eq!(canary_verdict(5_000, 1_000, true), CanaryVerdict::Rollback);

        let row = |skill: &str, model: &str, state| ModelCompile {
            skill_ref: skill.into(),
            model_ref: model.into(),
            compiled_digest: "d".into(),
            state,
        };
        let rows = [
            row("a", "m1", CompileState::Current),
            row("b", "m1", CompileState::Current),
            row("c", "m2", CompileState::Current),
            row("d", "m1", CompileState::Stale),
        ];
        let stale = stale_after_change(&rows, "m1", "a");
        assert_eq!(stale.len(), 1);
        assert_eq!(
            (stale[0].skill_ref.as_str(), stale[0].state),
            ("b", CompileState::Stale)
        );
    }
}
