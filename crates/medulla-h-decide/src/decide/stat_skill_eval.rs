//! The skill evaluation job behind `DecisionEval` op `skill_eval`
//! (MEDULLA-DECISION-ENGINE-R711.3 to R711.5).
//!
//! The job reads the cases and the clean-room runs of one skill revision, refuses what the
//! kernels refuse (an uncalibrated judge, a dirty run, an unrunnable case), scores outcome and
//! trajectory apart, stores each admitted result and the suite report by compare-and-set, and
//! answers the premises of the sealed record. A refused result is never stored. A saturated suite
//! is flagged and never ends in a promotion verdict by itself.

use std::collections::{BTreeMap, BTreeSet};

use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::encode_artifact;
use {
    medulla_decision_policy_types::skill_eval::EvalCaseKind,
    medulla_decision_policy_types::skill_eval::JudgeCalibration,
    medulla_decision_policy_types::skill_eval::RefusedEvalCase,
    medulla_decision_policy_types::skill_eval::SkillEvalCalibrateRequest,
    medulla_decision_policy_types::skill_eval::SkillEvalCalibrated,
    medulla_decision_policy_types::skill_eval::SkillEvalCase,
    medulla_decision_policy_types::skill_eval::SkillEvalCaseAddRequest,
    medulla_decision_policy_types::skill_eval::SkillEvalCaseAdded,
    medulla_decision_policy_types::skill_eval::SkillEvalOutcome,
    medulla_decision_policy_types::skill_eval::SkillEvalReportAnswer,
    medulla_decision_policy_types::skill_eval::SkillEvalReportRequest,
    medulla_decision_policy_types::skill_eval::SkillEvalRequest,
    medulla_decision_policy_types::skill_eval::SkillEvalResult,
    medulla_decision_policy_types::skill_eval::StoredSkillEvalResult,
    medulla_decision_policy_types::skill_eval::SuiteBand,
    medulla_decision_policy_types::skill_eval::SuiteReport,
    medulla_decision_policy_types::skill_eval::EVAL_CASE_INVALID,
    medulla_decision_policy_types::skill_eval::EVAL_SCOPE_REQUIRED,
    medulla_decision_policy_types::skill_eval::JUDGE_MIN_ANNOTATED,
    medulla_decision_policy_types::skill_eval::SKILL_EVAL_CASE_CLASS,
};
use {
    medulla_decision_stats_records::decision::skill_eval::check_case,
    medulla_decision_stats_records::decision::skill_eval::check_clean_room,
    medulla_decision_stats_records::decision::skill_eval::check_judge,
    medulla_decision_stats_records::decision::skill_eval::mix_in_band,
    medulla_decision_stats_records::decision::skill_eval::outcome_score,
    medulla_decision_stats_records::decision::skill_eval::pass_band,
    medulla_decision_stats_records::decision::skill_eval::suite_mix,
    medulla_decision_stats_records::decision::skill_eval::trajectory_score,
    medulla_decision_stats_records::decision::skill_eval::EvalKnobs,
    medulla_decision_stats_records::decision::skill_eval::EvalRefusal,
};

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
/// The store key prefix of the judge calibration of one skill's suite.
const CALIBRATION_PREFIX: &str = "judge-calibration:";

/// Refuse a caller that lacks `scope` (MEDULLA-DECISION-ENGINE-R711.7).
pub(super) fn require_scope(verified: &VerifiedRequestContext, scope: &str) -> Result<(), String> {
    if verified.allows_action(scope) {
        Ok(())
    } else {
        Err(format!(
            "{}: this op needs the scope {scope}",
            EVAL_SCOPE_REQUIRED.as_str()
        ))
    }
}

/// The judge calibration on record for `skill_ref` in `tenant`, when there is one.
pub(super) fn read_calibration(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
) -> Result<Option<JudgeCalibration>, String> {
    store
        .decision_artifact(tenant, &format!("{CALIBRATION_PREFIX}{skill_ref}"))?
        .map(|bytes| {
            medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "judge calibration")
        })
        .transpose()
}

/// Answer `skill_eval_report`: the latest report and calibration of the tenant's own suite. The
/// store is keyed by tenant, so a skill with no suite of this tenant answers the same whether
/// another tenant has one or the skill exists at all.
pub(super) fn report_answer(
    store: &AgentLibraryStore,
    request: &SkillEvalReportRequest,
) -> Result<SkillEvalReportAnswer, String> {
    Ok(SkillEvalReportAnswer {
        skill_ref: request.skill_ref.clone(),
        report: read_report(store, &request.tenant_id, &request.skill_ref)?,
        calibration: read_calibration(store, &request.tenant_id, &request.skill_ref)?,
    })
}

/// Record the judge calibration of one skill's suite and say whether it admits judged cases.
/// A later `skill_eval` that sends no calibration of its own reads this one.
pub(super) fn calibrate(
    store: &AgentLibraryStore,
    request: &SkillEvalCalibrateRequest,
) -> Result<SkillEvalCalibrated, String> {
    let knobs = knobs();
    if request.skill_ref.is_empty() || request.calibration.judge_ref.is_empty() {
        return Err(invalid_parameter(
            "a calibration names its skill and its judge",
        ));
    }
    let key = format!("{CALIBRATION_PREFIX}{}", request.skill_ref);
    swap(
        store,
        &request.tenant_id,
        &key,
        encode_artifact(&request.calibration)?,
    )?;
    Ok(SkillEvalCalibrated {
        skill_ref: request.skill_ref.clone(),
        admitted: request.calibration.kappa_ppm >= knobs.judge_min_kappa_ppm
            && request.calibration.n >= JUDGE_MIN_ANNOTATED,
        calibration: request.calibration.clone(),
    })
}

/// The graph node id of one case: namespaced by the tenant with its length in front, so no
/// tenant and case pair spells another's and a foreign tenant's case id never collides with it
/// (no existence oracle).
pub(super) fn case_node_id(tenant: &str, case_id: &str) -> String {
    format!("skill-eval-case:{}:{tenant}:{case_id}", tenant.len())
}

/// Plan `skill_eval_case_add`: check the case as the suite run checks it, then one
/// `SkillEvalCase` node owned by the caller in the caller's tenant. Adding a case id the tenant
/// already holds replaces that case only.
pub(super) fn case_add_operations(
    verified: &VerifiedRequestContext,
    request: &SkillEvalCaseAddRequest,
) -> Result<(Vec<Value>, SkillEvalCaseAdded), String> {
    let case = &request.case;
    if case.case_id.is_empty() || case.skill_ref.is_empty() || case.input_digest.is_empty() {
        return Err(invalid_parameter(
            "a case names its id, its skill and its input digest",
        ));
    }
    let tools: BTreeSet<String> = request.known_tools.iter().cloned().collect();
    check_case(case, &tools)
        .map_err(|refusal| format!("{}: {}", refusal.code.as_str(), refusal.reason))?;
    let node_id = case_node_id(&request.tenant_id, &case.case_id);
    let mut properties = match serde_json::to_value(case) {
        Ok(Value::Object(fields)) => fields,
        _ => return Err(invalid_parameter("the case does not encode")),
    };
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let kind = text(properties.get("kind"));
    let source = text(properties.get("source"));
    properties.insert("type".into(), SKILL_EVAL_CASE_CLASS.into());
    properties.insert("node_type".into(), SKILL_EVAL_CASE_CLASS.into());
    properties.insert("case_kind".into(), kind.into());
    properties.insert("case_source".into(), source.into());
    properties.insert("tenant_id".into(), request.tenant_id.clone().into());
    properties.insert("_owner".into(), verified.agent_id().into());
    let operations = vec![json!({"op": "upsert_node", "id": node_id, "properties": properties})];
    Ok((
        operations,
        SkillEvalCaseAdded {
            case_id: case.case_id.clone(),
            skill_ref: case.skill_ref.clone(),
            node_id,
        },
    ))
}

/// The latest stored suite report of `skill_ref` in `tenant`, when the skill has one.
pub(super) fn read_report(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
) -> Result<Option<SuiteReport>, String> {
    store
        .decision_artifact(tenant, &format!("{REPORT_PREFIX}{skill_ref}"))?
        .map(|bytes| {
            medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "suite report")
        })
        .transpose()
}

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
    if let Some(stray) = request
        .cases
        .iter()
        .find(|case| case.skill_ref != request.skill_ref)
    {
        return Err(invalid_parameter(format!(
            "case {} tests {}, not {}",
            stray.case_id, stray.skill_ref, request.skill_ref
        )));
    }
    let tools: BTreeSet<String> = request.known_tools.iter().cloned().collect();
    // A request that sends no calibration reads the one on record (`skill_eval_calibrate`).
    let calibration = match &request.calibration {
        Some(own) => Some(own.clone()),
        None => read_calibration(store, &request.tenant_id, &request.skill_ref)?,
    };
    let runs: BTreeMap<&str, _> = request
        .runs
        .iter()
        .map(|run| (run.case_id.as_str(), run))
        .collect();

    let mut stored = Vec::new();
    let mut refused = Vec::new();
    let mut admitted_cases: Vec<&SkillEvalCase> = Vec::new();
    for case in &request.cases {
        let verdict = admit(case, &tools, calibration.as_ref(), &knobs);
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
    let pass_rate_ppm = (passed * PPM).checked_div(n).unwrap_or(0);
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
            calibration.as_ref().map_or(0, |judge| judge.kappa_ppm)
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
        let key = format!(
            "{RESULT_PREFIX}{}:{}",
            request.skill_ref, row.result.case_id
        );
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
    calibration: Option<&JudgeCalibration>,
    knobs: &EvalKnobs,
) -> Result<(), EvalRefusal> {
    check_case(case, tools)?;
    check_judge(case, calibration, knobs)
}

fn swap(store: &AgentLibraryStore, tenant: &str, key: &str, bytes: Vec<u8>) -> Result<(), String> {
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
    let mut case_ids: Vec<&str> = request
        .cases
        .iter()
        .map(|case| case.case_id.as_str())
        .collect();
    case_ids.sort_unstable();
    let ids: Vec<&[u8]> = case_ids.iter().map(|id| id.as_bytes()).collect();
    let suite = format!(
        "sha256:{}",
        Digest256::framed(b"medulla.skill_eval.suite.v1", &ids)?.to_hex()
    );
    let passed = outcome
        .results
        .iter()
        .filter(|row| row.result.passed)
        .count() as u64;
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
    let node = format!(
        "skill-eval-report:{}:{}",
        request.skill_ref,
        &receipt_digest[7..23]
    );
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
        operations.push(edge_operation(
            &node,
            &request.skill_ref,
            EVALUATES,
            &receipt_digest,
        ));
    }
    Ok(operations)
}
