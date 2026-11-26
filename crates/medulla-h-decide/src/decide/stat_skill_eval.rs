//! The skill evaluation job behind `DecisionEval` op `skill_eval`
//! (MEDULLA-DECISION-ENGINE-R711.3 to R711.5).
//!
//! The job reads the cases and the clean-room runs of one skill revision, refuses what the
//! kernels refuse (an uncalibrated judge, a dirty run, an unrunnable case), scores outcome and
//! trajectory apart, stores each admitted result and the suite report by compare-and-set, and
//! answers the premises of the sealed record. A refused result is never stored. A saturated suite
//! is flagged and never ends in a promotion verdict by itself.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_stats::decision::skill_eval::{
    check_case, check_clean_room, check_judge, mix_in_band, outcome_score, pass_band, suite_mix,
    trajectory_score, EvalKnobs, EvalRefusal,
};
use medulla_decision_types::skill_eval::{
    EvalCaseKind, RefusedEvalCase, SkillEvalCase, SkillEvalOutcome, SkillEvalRequest,
    SkillEvalResult, StoredSkillEvalResult, SuiteBand, SuiteReport, EVAL_CASE_INVALID,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::encode_artifact;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::Digest256;
use serde_json::{json, Value};

use super::knowledge_flag::served::edge_operation;
use super::stat_support::{filtered_view_versioned, invalid_parameter};
use super::SharedState;

/// The graph class of a suite report node (vocabulary R817.4).
const SKILL_EVAL_REPORT_CLASS: &str = "SkillEvalReport";
/// The link from a suite report to the skill revision it evaluated.
const EVALUATES: &str = "evaluates";

const PPM: u64 = 1_000_000;
/// The store key prefix of one stored result.
const RESULT_PREFIX: &str = "skill-eval-result:";
/// The store key prefix of one suite report.
const REPORT_PREFIX: &str = "skill-eval-report:";

fn knobs() -> EvalKnobs {
    let knobs = medulla_server_api::knob_config::knobs();
    EvalKnobs {
        low_ppm: knobs.eval_pass_band_low_ppm,
        high_ppm: knobs.eval_pass_band_high_ppm,
        golden_ppm: knobs.eval_mix_golden_ppm,
        real_ppm: knobs.eval_mix_real_ppm,
        judged_ppm: knobs.eval_mix_judged_ppm,
        tolerance_ppm: knobs.eval_mix_tolerance_ppm,
        min_cases: knobs.eval_min_cases,
        judge_min_kappa_ppm: knobs.eval_judge_min_kappa_ppm,
    }
}

/// Run the suite of `request` and store what it admits.
pub(super) fn run(
    store: &AgentLibraryStore,
    request: &SkillEvalRequest,
) -> Result<SkillEvalOutcome, String> {
    let knobs = knobs();
    if request.cases.is_empty() {
        return Err(invalid_parameter("a suite holds at least one case"));
    }
    if let Some(stray) = request.cases.iter().find(|case| case.skill_ref != request.skill_ref) {
        return Err(invalid_parameter(format!(
            "case {} tests {}, not {}",
            stray.case_id, stray.skill_ref, request.skill_ref
        )));
    }
    let tools: BTreeSet<String> = request.known_tools.iter().cloned().collect();
    let runs: BTreeMap<&str, _> = request
        .runs
        .iter()
        .map(|run| (run.case_id.as_str(), run))
        .collect();

    let mut stored = Vec::new();
    let mut refused = Vec::new();
    let mut admitted_cases: Vec<&SkillEvalCase> = Vec::new();
    for case in &request.cases {
        let verdict = admit(case, &tools, request, &knobs);
        let run = runs.get(case.case_id.as_str());
        match (verdict, run) {
            (Err(refusal), _) => refused.push(RefusedEvalCase {
                case_id: case.case_id.clone(),
                code: refusal.code.as_str().to_string(),
            }),
            (Ok(()), None) => refused.push(RefusedEvalCase {
                case_id: case.case_id.clone(),
                code: EVAL_CASE_INVALID.as_str().to_string(),
            }),
            (Ok(()), Some(run)) => {
                if let Err(refusal) = check_clean_room(&run.attestation) {
                    refused.push(RefusedEvalCase {
                        case_id: case.case_id.clone(),
                        code: refusal.code.as_str().to_string(),
                    });
                    continue;
                }
                let outcome = outcome_score(case.expected_digest.as_deref(), &run.actual_digest);
                let trajectory = (!case.expected_trajectory.is_empty())
                    .then(|| trajectory_score(&case.expected_trajectory, &run.actual_trajectory));
                let passed = match case.kind {
                    EvalCaseKind::AuthorJudged => run.judged_passed.unwrap_or(false),
                    EvalCaseKind::Golden => outcome == PPM,
                    EvalCaseKind::RealPrompt => {
                        trajectory == Some(PPM)
                            && (case.expected_digest.is_none() || outcome == PPM)
                    }
                };
                admitted_cases.push(case);
                stored.push(StoredSkillEvalResult {
                    result: SkillEvalResult {
                        case_id: case.case_id.clone(),
                        outcome_score_ppm: outcome,
                        trajectory_score_ppm: trajectory,
                        passed,
                        attestation: run.attestation.clone(),
                    },
                    run_trace_id: run.run_trace_id.clone(),
                });
            }
        }
    }

    let n = stored.len() as u64;
    let passed = stored.iter().filter(|row| row.result.passed).count() as u64;
    let pass_rate_ppm = if n == 0 { 0 } else { passed * PPM / n };
    let owned: Vec<SkillEvalCase> = admitted_cases.iter().map(|case| (*case).clone()).collect();
    let mix = suite_mix(&owned);
    let band = pass_band(pass_rate_ppm, n, &knobs);
    let report = SuiteReport {
        golden_ppm: mix.0,
        real_ppm: mix.1,
        judged_ppm: mix.2,
        pass_rate_ppm,
        band,
        case_count: n,
    };

    // The sealed record's premises (R711.5).
    let count = |kind: EvalCaseKind| owned.iter().filter(|case| case.kind == kind).count();
    let mut premises = vec![
        format!("eval:band={}", band_text(band)),
        format!("eval:pass={passed}/{n}"),
        format!(
            "eval:mix={}/{}/{}",
            count(EvalCaseKind::Golden),
            count(EvalCaseKind::RealPrompt),
            count(EvalCaseKind::AuthorJudged)
        ),
        format!(
            "eval:judge_kappa={}",
            request.calibration.as_ref().map_or(0, |judge| judge.kappa_ppm)
        ),
    ];
    if band == SuiteBand::Saturated {
        premises.push("eval:suspicious_saturation".to_string());
    }
    if !mix_in_band(mix, &knobs) {
        premises.push("eval:mix_out_of_band".to_string());
    }

    // One owner write per row; each replaces the row of an earlier run of the same case.
    let tenant = &request.tenant_id;
    for row in &stored {
        let key = format!("{RESULT_PREFIX}{}:{}", request.skill_ref, row.result.case_id);
        swap(store, tenant, &key, encode_artifact(row)?)?;
    }
    let report_key = format!("{REPORT_PREFIX}{}", request.skill_ref);
    swap(store, tenant, &report_key, encode_artifact(&report)?)?;

    Ok(SkillEvalOutcome {
        skill_ref: request.skill_ref.clone(),
        results: stored,
        refused,
        report,
        premises,
    })
}

fn admit(
    case: &SkillEvalCase,
    tools: &BTreeSet<String>,
    request: &SkillEvalRequest,
    knobs: &EvalKnobs,
) -> Result<(), EvalRefusal> {
    check_case(case, tools)?;
    check_judge(case, request.calibration.as_ref(), knobs)
}

fn swap(
    store: &AgentLibraryStore,
    tenant: &str,
    key: &str,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let current = store.decision_artifact(tenant, key)?;
    if current.as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    store.swap_decision_artifact(tenant, key, current, bytes)
}

fn band_text(band: SuiteBand) -> &'static str {
    match band {
        SuiteBand::Healthy => "healthy",
        SuiteBand::TooLow => "too_low",
        SuiteBand::Saturated => "saturated",
    }
}

/// The batch that records one suite run in the graph: a `SkillEvalReport` node holding the suite,
/// the skill revision, the pass and fail counts and the receipt digest, linked to the skill node
/// with `evaluates` when the caller's view holds it (MEDULLA-ORCH-VOCAB-R817.4).
///
/// The node id carries the receipt digest, so rerunning an unchanged suite upserts the same node
/// and a changed suite adds a newer one; the latest report of a skill is the one with the largest
/// `evaluated_ms`.
pub(super) async fn report_operations(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
    request: &SkillEvalRequest,
    outcome: &SkillEvalOutcome,
    now_ms: u64,
) -> Result<Vec<Value>, String> {
    let mut case_ids: Vec<&str> = request.cases.iter().map(|case| case.case_id.as_str()).collect();
    case_ids.sort_unstable();
    let ids: Vec<&[u8]> = case_ids.iter().map(|id| id.as_bytes()).collect();
    let suite = format!(
        "sha256:{}",
        Digest256::framed(b"medulla.skill_eval.suite.v1", &ids)?.to_hex()
    );
    let passed = outcome.results.iter().filter(|row| row.result.passed).count() as u64;
    let admitted = outcome.results.len() as u64;
    let report_bytes = encode_artifact(&outcome.report)?;
    let receipt_digest = format!(
        "sha256:{}",
        Digest256::framed(
            b"medulla.skill_eval.report.v1",
            &[
                request.skill_ref.as_bytes(),
                suite.as_bytes(),
                report_bytes.as_slice(),
            ],
        )?
        .to_hex()
    );
    let node = format!("skill-eval-report:{}:{}", request.skill_ref, &receipt_digest[7..23]);
    let mut operations = vec![json!({
        "op": "upsert_node",
        "id": node,
        "properties": {
            "type": SKILL_EVAL_REPORT_CLASS,
            "node_type": SKILL_EVAL_REPORT_CLASS,
            "tenant_id": request.tenant_id,
            "evaluated_skill": request.skill_ref,
            "suite": suite,
            "pass_count": passed,
            "fail_count": admitted - passed,
            "case_count": outcome.report.case_count,
            "pass_rate_ppm": outcome.report.pass_rate_ppm,
            "band": band_text(outcome.report.band),
            "receipt_digest": receipt_digest,
            "evaluated_ms": now_ms,
            "receipt_id": receipt_digest,
        },
    })];
    let (view, _) = filtered_view_versioned(state, verified.agent_id(), graph).await?;
    let skill_known = view.node_row_object(&request.skill_ref).is_some_and(|row| {
        row.get("type").and_then(Value::as_str) == Some("Skill")
            && row
                .get("tenant_id")
                .and_then(Value::as_str)
                .is_none_or(|owner| owner == request.tenant_id)
    });
    if skill_known {
        operations.push(edge_operation(&node, &request.skill_ref, EVALUATES, &receipt_digest));
    }
    Ok(operations)
}
