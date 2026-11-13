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
//! enter through `DecisionLog` `Evaluate`; `Decide` writes nothing.

use std::collections::BTreeMap;

use medulla_decision_rank::rank::path_success::rank_paths;
use medulla_decision_types::decision::jobs::RecordWindow;
use medulla_decision_types::decision::path_success::{PathPreferenceRule, SuccessRateEvidence};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::log::OutcomeAggregateRequest;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_log::{aggregate_log, recent_evaluations, LogReader};
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
    for option in options.iter() {
        let path_id = option.option_id.as_str();
        // The cross-question row of the path; a path below the aggregate's
        // support floor has no row and so no evidence.
        let Some(row) = aggregate
            .rows
            .iter()
            .find(|row| row.option_id == path_id && row.question_id.is_none())
        else {
            continue;
        };
        evidence.push(SuccessRateEvidence {
            path_id: path_id.to_string(),
            successful_attempts: u32::try_from(row.successes).unwrap_or(u32::MAX),
            total_attempts: u32::try_from(row.trials).unwrap_or(u32::MAX),
            window_ms: rule.window_ms,
        });
        recent.insert(
            path_id.to_string(),
            recent_evaluations(
                ctx.store,
                reader,
                path_id,
                rule.skip_after_failures as usize,
            )?,
        );
    }
    let ranked = rank_paths(&evidence, &recent, &rule);
    let mut executed = advisory(ranked.iter().map(|path| path.path_id.clone()).collect())?;
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
