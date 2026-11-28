//! The `skill_eval` job of `DecisionEval` (MEDULLA-DECISION-ENGINE-R711.3 to R711.5): results and
//! the suite report are stored, a refused result is not, and a saturated suite is flagged.

use medulla_h_decide_learning::decide::stat_skill_eval::run;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use {
    medulla_decision_policy_types::skill_eval::EnvironmentAttestation,
    medulla_decision_policy_types::skill_eval::EvalCaseKind,
    medulla_decision_policy_types::skill_eval::EvalCaseSource,
    medulla_decision_policy_types::skill_eval::JudgeCalibration,
    medulla_decision_policy_types::skill_eval::SkillEvalCase,
    medulla_decision_policy_types::skill_eval::SkillEvalOutcome,
    medulla_decision_policy_types::skill_eval::SkillEvalRequest,
    medulla_decision_policy_types::skill_eval::SkillEvalRun,
    medulla_decision_policy_types::skill_eval::SuiteBand,
    medulla_decision_policy_types::skill_eval::TrajectoryKind,
    medulla_decision_policy_types::skill_eval::TrajectoryStep,
    medulla_decision_policy_types::skill_eval::EVAL_RUN_NOT_CLEAN_ROOM,
    medulla_decision_policy_types::skill_eval::JUDGE_NOT_CALIBRATED,
};

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
                actual_digest: if ok {
                    format!("out-{index}")
                } else {
                    "wrong".into()
                },
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

async fn store(
    h: &Harness,
) -> std::sync::Arc<medulla_persistence_records::agent_library::AgentLibraryStore> {
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
    assert_eq!(
        outcome.results[5].result.trajectory_score_ppm,
        Some(1_000_000)
    );
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
    assert!(healthy
        .premises
        .contains(&"eval:judge_kappa=700000".to_string()));
    assert!(!healthy
        .premises
        .contains(&"eval:suspicious_saturation".to_string()));

    // Every case passing is saturation: it is flagged and never a verdict by itself.
    let saturated = run(&store, &request(0, 700_000)).expect("reruns");
    assert_eq!(saturated.report.band, SuiteBand::Saturated);
    assert!(saturated
        .premises
        .contains(&"eval:band=saturated".to_string()));
    assert!(saturated
        .premises
        .contains(&"eval:suspicious_saturation".to_string()));
    assert!(saturated
        .premises
        .iter()
        .all(|premise| !premise.contains("promote")));

    // A mix far from the wanted shares is flagged too.
    let mut skewed = request(4, 700_000);
    for case in skewed.cases.iter_mut().skip(1) {
        case.kind = EvalCaseKind::Golden;
        case.expected_trajectory.clear();
    }
    let flagged = run(&store, &skewed).expect("runs");
    assert!(flagged
        .premises
        .contains(&"eval:mix_out_of_band".to_string()));
}

// spec: MEDULLA-DECISION-ENGINE-R711.3
#[tokio::test]
async fn report_is_a_graph_node_with_counts_and_a_receipt_digest() {
    use medulla_h_decide_learning::decide::stat_skill_eval::report_operations;
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let store = store(&h).await;
    let core = h
        .state
        .read()
        .await
        .registry
        .get("__commons__")
        .expect("graph exists")
        .core
        .clone();
    core.add_node(
        SKILL.to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({"type": "Skill", "tenant_id": TENANT}))
            .unwrap(),
    );
    let who = VerifiedRequestContext::verified_for_test_with_scopes("agent-7", TENANT, &["*"]);
    let req = request(4, 700_000);
    let outcome = run(&store, &req).expect("runs");
    let operations = report_operations(&h.state, &who, "__commons__", &req, &outcome, 77)
        .await
        .expect("plans");
    let node = &operations[0];
    assert_eq!(node["op"], "upsert_node");
    let props = &node["properties"];
    assert_eq!(props["type"], "SkillEvalReport");
    assert_eq!(props["evaluated_skill"], SKILL);
    assert_eq!(props["pass_count"], 16);
    assert_eq!(props["fail_count"], 4);
    assert_eq!(props["evaluated_ms"], 77);
    assert!(props["receipt_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(props["suite"].as_str().unwrap().starts_with("sha256:"));
    // The report links to the skill revision it evaluated.
    assert_eq!(operations[1]["op"], "upsert_edge");
    assert_eq!(operations[1]["target"], SKILL);
    // The same suite run again plans the same node.
    let again = run(&store, &req).expect("reruns");
    let repeat = report_operations(&h.state, &who, "__commons__", &req, &again, 99)
        .await
        .expect("plans");
    assert_eq!(repeat[0]["id"], node["id"]);
}

fn eval_caller(tenant: &str, scopes: &[&str]) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("agent-7", tenant, scopes)
}

// spec: MEDULLA-DECISION-ENGINE-R711.7
#[tokio::test]
async fn a_case_is_added_checked_and_scoped_to_its_tenant() {
    use {
        medulla_decision_policy_types::skill_eval::SkillEvalCaseAddRequest,
        medulla_decision_policy_types::skill_eval::SCOPE_AUTHOR,
    };
    use {
        medulla_h_decide_learning::decide::stat_skill_eval::case_add_operations,
        medulla_h_decide_learning::decide::stat_skill_eval::case_node_id,
        medulla_h_decide_learning::decide::stat_skill_eval::require_scope,
    };
    let add = |case: SkillEvalCase, tenant: &str| SkillEvalCaseAddRequest {
        tenant_id: tenant.into(),
        case,
        known_tools: vec!["search".into()],
    };
    let author = eval_caller(TENANT, &[SCOPE_AUTHOR]);
    // The scope gate refuses a caller without `skills:author` and admits one with it.
    let reader = eval_caller(TENANT, &["skills:eval"]);
    let refused = require_scope(&reader, SCOPE_AUTHOR).expect_err("the author scope is required");
    assert!(refused.starts_with("EVAL_SCOPE_REQUIRED"), "{refused}");
    require_scope(&author, SCOPE_AUTHOR).expect("admitted");

    // A valid case plans one tenant-namespaced node that carries the vocabulary properties.
    let good = case(1, EvalCaseKind::RealPrompt);
    let (operations, added) =
        case_add_operations(&author, &add(good.clone(), TENANT)).expect("plans");
    assert_eq!(added.node_id, case_node_id(TENANT, "case-1"));
    let props = &operations[0]["properties"];
    assert_eq!(operations[0]["id"], added.node_id);
    assert_eq!(props["type"], "SkillEvalCase");
    assert_eq!(props["case_kind"], "real_prompt");
    assert_eq!(props["case_source"], "authored");
    assert_eq!(props["tenant_id"], TENANT);
    assert_eq!(props["_owner"], "agent-7");
    // The same case id of another tenant is another node: no collision, no oracle.
    assert_ne!(case_node_id("tenant-b", "case-1"), added.node_id);

    // The run's own checks refuse an unrunnable case: a golden case with no expected digest,
    // a real-prompt case with no trajectory and a mock of an unknown tool.
    let mut golden = case(2, EvalCaseKind::Golden);
    golden.expected_digest = None;
    let mut bare = case(3, EvalCaseKind::RealPrompt);
    bare.expected_trajectory.clear();
    let mut mocked = case(4, EvalCaseKind::Golden);
    mocked.tool_mocks = vec![medulla_decision_policy_types::skill_eval::ToolMock {
        tool: "unknown".into(),
        args_digest: "a".into(),
        result_digest: "b".into(),
    }];
    for bad in [golden, bare, mocked] {
        let error = case_add_operations(&author, &add(bad, TENANT)).expect_err("refused");
        assert!(error.starts_with("EVAL_CASE_INVALID"), "{error}");
    }
}

// spec: MEDULLA-DECISION-ENGINE-R711.7
#[tokio::test]
async fn report_and_calibration_are_read_and_recorded_per_tenant() {
    use {
        medulla_decision_policy_types::skill_eval::SkillEvalCalibrateRequest,
        medulla_decision_policy_types::skill_eval::SkillEvalReportRequest,
    };
    use {
        medulla_h_decide_learning::decide::stat_skill_eval::calibrate,
        medulla_h_decide_learning::decide::stat_skill_eval::report_answer,
    };
    let h = Harness::new().await;
    let store = store(&h).await;
    let ask = |tenant: &str| SkillEvalReportRequest {
        tenant_id: tenant.into(),
        skill_ref: SKILL.into(),
    };

    // No suite yet: the answer is empty, and the same for another tenant and for a skill that
    // does not exist, so it says nothing about what exists.
    let empty = report_answer(&store, &ask(TENANT)).expect("reads");
    assert_eq!(
        (empty.report.clone(), empty.calibration.clone()),
        (None, None)
    );

    // Calibrate below the bound: recorded, not admitted. At the bound and enough items: admitted.
    let record = |kappa_ppm: u64, n: u64| SkillEvalCalibrateRequest {
        tenant_id: TENANT.into(),
        skill_ref: SKILL.into(),
        calibration: JudgeCalibration {
            judge_ref: "judge-1".into(),
            annotated_set_ref: "set-1".into(),
            kappa_ppm,
            n,
        },
    };
    assert!(
        !calibrate(&store, &record(100_000, 60))
            .expect("records")
            .admitted
    );
    assert!(
        !calibrate(&store, &record(900_000, 10))
            .expect("records")
            .admitted
    );
    let admitted = calibrate(&store, &record(900_000, 60)).expect("records");
    assert!(admitted.admitted);

    // A suite run that sends no calibration reads the one on record and runs its judged cases.
    let mut req = request(4, 0);
    req.calibration = None;
    let outcome = run(&store, &req).expect("runs with the recorded calibration");
    assert!(outcome.refused.is_empty(), "{:?}", outcome.refused);
    assert!(outcome
        .premises
        .contains(&"eval:judge_kappa=900000".to_string()));

    // The report read returns the stored report and calibration; another tenant sees none.
    let mine = report_answer(&store, &ask(TENANT)).expect("reads");
    assert_eq!(mine.report, Some(outcome.report));
    assert_eq!(mine.calibration, Some(admitted.calibration));
    let theirs = report_answer(&store, &ask("tenant-b")).expect("reads");
    assert_eq!((theirs.report, theirs.calibration), (None, None));
}
