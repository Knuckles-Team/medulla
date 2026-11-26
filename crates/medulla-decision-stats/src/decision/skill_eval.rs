//! Pure skill-evaluation kernels (MEDULLA-DECISION-ENGINE-R711.2, R711.4).
//!
//! No I/O and no clock. [`trajectory_score`] and the outcome score are separate numbers that
//! never merge; [`suite_mix`] and [`pass_band`] describe a suite; the checks refuse a judged
//! case under an uncalibrated judge, a result of a run that read the host, and a case the kind or
//! its mocks make unrunnable. All shares are integer parts per million.

use std::collections::BTreeSet;

use medulla_decision_types::skill_eval::{
    EnvironmentAttestation, EvalCaseKind, JudgeCalibration, SkillEvalCase, SuiteBand,
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
        None => Err(refuse(JUDGE_NOT_CALIBRATED, "the case names no calibrated judge")),
    }
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
        return Err(refuse(EVAL_CASE_INVALID, "a golden case names no expected_digest"));
    }
    if case.kind == EvalCaseKind::RealPrompt && case.expected_trajectory.is_empty() {
        return Err(refuse(
            EVAL_CASE_INVALID,
            "a real-prompt case names no expected_trajectory",
        ));
    }
    if let Some(mock) = case.tool_mocks.iter().find(|mock| !known_tools.contains(&mock.tool)) {
        return Err(refuse(
            EVAL_CASE_INVALID,
            format!("the case mocks unknown tool {}", mock.tool),
        ));
    }
    Ok(())
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
            trajectory_score(&[step(TrajectoryKind::Skill, "a")], &[step(TrajectoryKind::Tool, "a")]),
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
        assert_eq!(check_clean_room(&attestation(1)).unwrap_err().code, EVAL_RUN_NOT_CLEAN_ROOM);

        let tools: BTreeSet<String> = ["search".to_string()].into();
        let mut golden = case(EvalCaseKind::Golden);
        golden.expected_digest = None;
        assert_eq!(check_case(&golden, &tools).unwrap_err().code, EVAL_CASE_INVALID);
        let mut real = case(EvalCaseKind::RealPrompt);
        real.expected_trajectory.clear();
        assert_eq!(check_case(&real, &tools).unwrap_err().code, EVAL_CASE_INVALID);
        let mut mocked = case(EvalCaseKind::RealPrompt);
        mocked.tool_mocks = vec![ToolMock {
            tool: "delete".into(),
            args_digest: "a".into(),
            result_digest: "r".into(),
        }];
        assert_eq!(check_case(&mocked, &tools).unwrap_err().code, EVAL_CASE_INVALID);
        assert!(check_case(&case(EvalCaseKind::RealPrompt), &tools).is_ok());
    }
}
