//! The `path_preference` question family (MEDULLA-DECISION-ENGINE-R159.3).
//!
//! The declared options are routing paths, each named by its `path_id`. The
//! family reads the caller's decision log: for each path the outcome counts of
//! the last `path.window_ms` come from `aggregate_log` (the cross-question row
//! of the option whose id is the `path_id`), and the latest evaluations for the
//! skip rule come from `recent_evaluations`. Only independently evaluated,
//! uncensored outcomes count (the one label rule). `rank_paths` orders the
//! eligible paths, and the answer is an advisory with one scored option per
//! eligible path, best first; the order is also the fallback order. Outcomes
//! enter through `DecisionLog` `Evaluate`; `Decide` writes nothing. A path with
//! no evaluation, none inside the window, or fewer attempts than
//! `path.min_samples` inside it is eliminated with its code
//! (MEDULLA-DECISION-ENGINE-R159.4); when none remains the answer abstains.

use std::collections::BTreeMap;

use medulla_decision_rank::rank::path_success::rank_paths;
use medulla_decision_types::decision::jobs::RecordWindow;
use medulla_decision_types::decision::path_success::{
    PathPreferenceRule, SuccessRateEvidence, PATH_OUTCOME_MISSING, PATH_UNMEASURED,
    PATH_WINDOW_STALE,
};
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::log::OutcomeAggregateRequest;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, QuantScaleTag,
    QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_log::{aggregate_log, recent_labels, LogReader};
use crate::decide::stat_support::refusal;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

use super::support::{advisory, invalid};
use super::DecisionFamily;

/// The family of the `path_preference` question kind.
pub struct PathPreferenceFamily;

/// A path id is the SHA-256 of its three parts: 64 lowercase hex characters.
fn is_path_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One ratio is `1_000_000` millionths; scores are Q32 ratios.
fn rate_score(rate_micros: u32) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: ((i128::from(rate_micros) << 32) / 1_000_000) as i64,
    }
}

fn conclude_preference(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid("a path_preference question declares its paths"));
    };
    let rule = PathPreferenceRule::from_policy(policy);
    let window = RecordWindow {
        from_ms: ctx.now_ms.saturating_sub(rule.window_ms),
        to_ms: ctx.now_ms,
    };
    let aggregate = aggregate_log(
        ctx.store,
        reader,
        &OutcomeAggregateRequest {
            tenant_id: ctx.tenant_id.to_string(),
            question_id: None,
            window,
            attribution: None,
            transfer: None,
        },
    )?;
    let mut evidence = Vec::new();
    let mut recent: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    // Each declared path's latest label: what its evidence premises cite.
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    // The paths the engine eliminates, in `path_id` order, with their code.
    let mut eliminated: Vec<(String, &'static str, String)> = Vec::new();
    for option in options.iter() {
        let path_id = option.option_id.as_str();
        let labels = recent_labels(
            ctx.store,
            reader,
            path_id,
            (rule.skip_after_failures as usize).max(1),
        )?;
        let Some(newest) = labels.first() else {
            eliminated.push((
                path_id.to_string(),
                PATH_UNMEASURED,
                "the path has no evaluation".to_string(),
            ));
            continue;
        };
        if newest.recorded_at_ms < window.from_ms {
            eliminated.push((
                path_id.to_string(),
                PATH_WINDOW_STALE,
                format!("no evaluation lies inside the last {} ms", rule.window_ms),
            ));
            continue;
        }
        // The cross-question row of the path; a path below the aggregate's
        // support floor has no row and so no evidence.
        let row = aggregate
            .rows
            .iter()
            .find(|row| row.option_id == path_id && row.question_id.is_none());
        let attempts = row.map_or(0, |row| row.trials);
        let Some(row) = row.filter(|_| attempts >= u64::from(rule.min_samples)) else {
            eliminated.push((
                path_id.to_string(),
                PATH_OUTCOME_MISSING,
                format!(
                    "{attempts} attempts lie inside the window, below path.min_samples {}",
                    rule.min_samples
                ),
            ));
            continue;
        };
        evidence.push(SuccessRateEvidence {
            path_id: path_id.to_string(),
            successful_attempts: u32::try_from(row.successes).unwrap_or(u32::MAX),
            total_attempts: u32::try_from(row.trials).unwrap_or(u32::MAX),
            window_ms: rule.window_ms,
        });
        latest.insert(path_id.to_string(), newest.evaluation_id.clone());
        recent.insert(
            path_id.to_string(),
            labels.iter().map(|label| label.success).collect(),
        );
    }
    let ranked = rank_paths(&evidence, &recent, &rule);
    let mut premises: Vec<FamilyPremise> = Vec::new();
    for item in &evidence {
        let id = item.path_id.as_str();
        let cited = latest.get(id).cloned().unwrap_or_default();
        let observed = |subject: &'static str, fact: String| FamilyPremise {
            subject,
            fact,
            class: PremiseClass::Observation,
            source: FamilyPremiseSource::Observation(cited.clone()),
        };
        premises.push(observed("path_id", format!("path:id={id}")));
        premises.push(observed(
            "path_rate",
            format!(
                "path:rate={}/{}",
                item.successful_attempts, item.total_attempts
            ),
        ));
        premises.push(observed(
            "path_window_ms",
            format!("path:window_ms={}", item.window_ms),
        ));
        let applied = ranked.iter().any(|path| path.path_id == id);
        premises.push(observed(
            "path_preference",
            format!(
                "path:preference={} path_id={id}",
                if applied { "applied" } else { "skipped" }
            ),
        ));
    }
    for (id, code, _) in &eliminated {
        premises.push(FamilyPremise {
            subject: "path_eliminated",
            fact: format!("path:eliminated={code} path_id={id}"),
            class: PremiseClass::Proof,
            source: FamilyPremiseSource::Request("candidates"),
        });
    }
    if ranked.is_empty() {
        if let Some((_, code, detail)) = eliminated.first() {
            let mut executed = advisory(Vec::new())?;
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::Family {
                    family: QuestionKind::PathPreference,
                    code: RefusalCode::new(code),
                    detail: detail.clone(),
                }])?,
            };
            executed.family_premises = premises;
            return Ok(executed);
        }
    }
    let mut executed = advisory(ranked.iter().map(|path| path.path_id.clone()).collect())?;
    executed.family_premises = premises;
    let scored: Vec<ScoredOption> = ranked
        .iter()
        .map(|path| ScoredOption {
            option_id: path.path_id.clone(),
            score: rate_score(path.rate_micros),
            probability: None,
        })
        .collect();
    if let StatisticalOutcome::Advisory { scores, .. } = &mut executed.ladder.outcome {
        *scores = BoundedVec::new(scored)
            .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    }
    Ok(executed)
}

impl DecisionFamily for PathPreferenceFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        match options.iter().find(|option| !is_path_id(&option.option_id)) {
            Some(option) => Err(invalid(format!(
                "option {} is not a path id (64 lowercase hex characters)",
                option.option_id
            ))),
            None => Ok(()),
        }
    }

    fn conclude_with_log(
        &self,
        ctx: &ExecutionContext,
        reader: &LogReader,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_preference(ctx, reader, request, policy))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
