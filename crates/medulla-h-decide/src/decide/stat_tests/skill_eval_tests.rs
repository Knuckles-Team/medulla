//! The `skill_eval` job of `DecisionEval` (MEDULLA-DECISION-ENGINE-R711.3 to R711.5): results and
//! the suite report are stored, a refused result is not, and a saturated suite is flagged.

use super::super::stat_skill_eval::run;
use super::*;
use medulla_decision_types::skill_eval::{
    EnvironmentAttestation, EvalCaseKind, EvalCaseSource, JudgeCalibration, SkillEvalCase,
    SkillEvalOutcome, SkillEvalRequest, SkillEvalRun, SuiteBand, TrajectoryKind, TrajectoryStep,
    EVAL_RUN_NOT_CLEAN_ROOM, JUDGE_NOT_CALIBRATED,
};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;

const SKILL: &str = "skill:triage@3";

fn trajectory() -> Vec<TrajectoryStep> {
    vec![TrajectoryStep {
        kind: TrajectoryKind::Tool,
        name: "search".into(),
    }]
}

fn case(index: usize, kind: EvalCaseKind) -> SkillEvalCase {
    SkillEvalCase {
        case_id: format!("case-{index}"),
        skill_ref: SKILL.into(),
        kind,
        input_digest: format!("in-{index}"),
        expected_digest: Some(format!("out-{index}")),
        tool_mocks: Vec::new(),
        expected_trajectory: match kind {
            EvalCaseKind::RealPrompt => trajectory(),
            _ => Vec::new(),
        },
        source: EvalCaseSource::Authored,
    }
}

/// 1 golden, 16 real-prompt and 3 author-judged cases.
fn suite() -> Vec<SkillEvalCase> {
    (0..20)
        .map(|index| {
            let kind = match index {
                0 => EvalCaseKind::Golden,
                1..=16 => EvalCaseKind::RealPrompt,
                _ => EvalCaseKind::AuthorJudged,
            };
            case(index, kind)
        })
        .collect()
}

fn attestation(host_paths_read: u64) -> EnvironmentAttestation {
    EnvironmentAttestation {
        sandbox_digest: "sandbox".into(),
        env_digest: "env".into(),
        host_paths_read,
    }
}

/// A run of every case; the first `failing` cases of the list answer wrongly.
fn runs(cases: &[SkillEvalCase], failing: usize) -> Vec<SkillEvalRun> {
    cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let ok = index >= failing;
            SkillEvalRun {
                case_id: case.case_id.clone(),
                actual_digest: if ok { format!("out-{index}") } else { "wrong".into() },
                actual_trajectory: if ok { trajectory() } else { Vec::new() },
                judged_passed: Some(ok),
                attestation: attestation(0),
                run_trace_id: Some(format!("trace-{index}")),
            }
        })
        .collect()
}

fn request(failing: usize, kappa_ppm: u64) -> SkillEvalRequest {
    let cases = suite();
    SkillEvalRequest {
        tenant_id: TENANT.into(),
        skill_ref: SKILL.into(),
        runs: runs(&cases, failing),
        cases,
        calibration: Some(JudgeCalibration {
            judge_ref: "judge-1".into(),
            annotated_set_ref: "set-1".into(),
            kappa_ppm,
            n: 60,
        }),
        known_tools: vec!["search".into()],
    }
}

async fn store(h: &Harness) -> std::sync::Arc<medulla_persistence_records::agent_library::AgentLibraryStore> {
    h.state.write().await.ensure_agent_library().unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R711.3
#[tokio::test]
async fn job_stores_results_and_report() {
    let h = Harness::new().await;
    let store = store(&h).await;
    // Four failing cases of twenty: 80 percent passes, which is inside the healthy band.
    let outcome: SkillEvalOutcome = run(&store, &request(4, 700_000)).expect("runs");
    assert_eq!(outcome.report.band, SuiteBand::Healthy);
    assert_eq!(outcome.report.case_count, 20);
    assert_eq!(outcome.report.pass_rate_ppm, 800_000);
    assert_eq!(outcome.results.len(), 20);
    // Outcome and trajectory scores are separate fields; the golden case has no trajectory.
    assert_eq!(outcome.results[0].result.trajectory_score_ppm, None);
    assert_eq!(outcome.results[5].result.trajectory_score_ppm, Some(1_000_000));
    // Each result links to its run trace and is stored with the report.
    assert_eq!(outcome.results[7].run_trace_id.as_deref(), Some("trace-7"));
    let rows = store
        .decision_artifacts_with_prefix(TENANT, "skill-eval-result:", 100)
        .unwrap();
    assert_eq!(rows.len(), 20);
    assert!(store
        .decision_artifact(TENANT, &format!("skill-eval-report:{SKILL}"))
        .unwrap()
        .is_some());
    // A rerun replaces the rows instead of adding to them.
    run(&store, &request(0, 700_000)).expect("reruns");
    assert_eq!(
        store
            .decision_artifacts_with_prefix(TENANT, "skill-eval-result:", 100)
            .unwrap()
            .len(),
        20
    );
}

// spec: MEDULLA-DECISION-ENGINE-R711.4
#[tokio::test]
async fn uncalibrated_judge_and_dirty_run_refused() {
    let h = Harness::new().await;
    let store = store(&h).await;
    let mut dirty = request(0, 400_000);
    dirty.runs[3].attestation = attestation(2);
    let outcome = run(&store, &dirty).expect("runs");
    let refused = |code: &str| -> Vec<&str> {
        outcome
            .refused
            .iter()
            .filter(|row| row.code == code)
            .map(|row| row.case_id.as_str())
            .collect()
    };
    // The three judged cases are refused under a judge of kappa 0.4.
    assert_eq!(
        refused(JUDGE_NOT_CALIBRATED.as_str()),
        ["case-17", "case-18", "case-19"]
    );
    assert_eq!(refused(EVAL_RUN_NOT_CLEAN_ROOM.as_str()), ["case-3"]);
    // None of the four refused results is stored.
    assert_eq!(outcome.results.len(), 16);
    let rows = store
        .decision_artifacts_with_prefix(TENANT, "skill-eval-result:", 100)
        .unwrap();
    assert_eq!(rows.len(), 16);
    assert!(rows.iter().all(|(key, _)| !key.ends_with("case-3")));
}

// spec: MEDULLA-DECISION-ENGINE-R711.5
#[tokio::test]
async fn saturation_is_flagged_not_trusted() {
    let h = Harness::new().await;
    let store = store(&h).await;
    let healthy = run(&store, &request(4, 700_000)).expect("runs");
    assert!(healthy.premises.contains(&"eval:band=healthy".to_string()));
    assert!(healthy.premises.contains(&"eval:pass=16/20".to_string()));
    assert!(healthy.premises.contains(&"eval:mix=1/16/3".to_string()));
    assert!(healthy.premises.contains(&"eval:judge_kappa=700000".to_string()));
    assert!(!healthy.premises.contains(&"eval:suspicious_saturation".to_string()));

    // Every case passing is saturation: it is flagged and never a verdict by itself.
    let saturated = run(&store, &request(0, 700_000)).expect("reruns");
    assert_eq!(saturated.report.band, SuiteBand::Saturated);
    assert!(saturated.premises.contains(&"eval:band=saturated".to_string()));
    assert!(saturated
        .premises
        .contains(&"eval:suspicious_saturation".to_string()));
    assert!(saturated.premises.iter().all(|premise| !premise.contains("promote")));

    // A mix far from the wanted shares is flagged too.
    let mut skewed = request(4, 700_000);
    for case in skewed.cases.iter_mut().skip(1) {
        case.kind = EvalCaseKind::Golden;
        case.expected_trajectory.clear();
    }
    let flagged = run(&store, &skewed).expect("runs");
    assert!(flagged.premises.contains(&"eval:mix_out_of_band".to_string()));
}
