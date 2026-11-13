//! The `context_budget` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R029.2.2).
//!
//! Which context sources go into the model window under a token budget. The
//! shared ladder reads the pinned head over the visible sources; its score is
//! each source's relevance. `refine` then places every source exactly once with
//! the partition kernel and answers the selected sources as advisory scores.
//! A source the caller named but may not read is reported `Unauthorized` by the
//! caller's own string only; an unnamed invisible source never enters a count.

use std::collections::BTreeSet;

use medulla_decision_outcome_types::decision::context_budget::{
    AvailableContextSource, ContextSourceSelection, OmissionReason, OmittedContextSource,
};
use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::context_partition::partition;
use medulla_decision_stats::decision::quant::q32;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, Pinned};
use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// Param naming the token budget (`Int`, positive).
const BUDGET_PARAM: &str = "context.token_budget";
/// Param naming the sources the caller expects (`IriList`).
const REQUESTED_PARAM: &str = "context.requested_sources";
/// Source fact: its token cost, on `Q32` (a whole number of tokens).
const TOKENS_KEY: &str = "context.tokens";
/// Source fact: the subject it speaks about.
const SUBJECT_KEY: &str = "context.subject";
/// Source fact: its authority class (`proof`, `observation`, `claim`).
const AUTHORITY_KEY: &str = "context.authority_class";
/// Domain of the source snapshot digest.
const SNAPSHOT_DOMAIN: &str = "eg/context-source-snapshot/v1";

/// The family of the `context_budget` question kind.
pub struct ContextBudgetFamily;

fn requested(request: &DecideRequest) -> Vec<String> {
    request
        .params
        .iter()
        .find(|param| param.name == REQUESTED_PARAM)
        .and_then(|param| match &param.value {
            TypedValue::IriList(names) => Some(names.iter().cloned().collect()),
            _ => None,
        })
        .unwrap_or_default()
}

fn budget_tokens(request: &DecideRequest) -> Result<u64, String> {
    let missing = || {
        refusal(
            StatisticalErrorCode::ContextBudgetMissing,
            format!("param {BUDGET_PARAM} must be a positive Int"),
        )
    };
    match request
        .params
        .iter()
        .find(|param| param.name == BUDGET_PARAM)
    {
        Some(param) => match &param.value {
            TypedValue::Int(tokens) if *tokens > 0 => Ok(*tokens as u64),
            _ => Err(missing()),
        },
        None => Err(missing()),
    }
}

/// A whole-token cost out of a `Q32` fact.
fn tokens_of(id: &str, q32_value: Option<i64>) -> Result<u64, String> {
    match q32_value {
        Some(value) if value >= 0 => Ok((value >> 32) as u64),
        _ => Err(refusal(
            StatisticalErrorCode::ContextTokensUnknown,
            format!("source {id} declares no {TOKENS_KEY}"),
        )),
    }
}

fn authority_rank(class: Option<&str>) -> u8 {
    match class.map(str::to_ascii_lowercase).as_deref() {
        Some("proof") => 3,
        Some("observation") => 2,
        Some("claim") => 1,
        _ => 0,
    }
}

fn first_duplicate<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = BTreeSet::new();
    names.into_iter().find(|name| !seen.insert(*name))
}

impl DecisionFamily for ContextBudgetFamily {
    fn precheck_declared(&self, options: &[DeclaredOption]) -> Result<(), String> {
        match first_duplicate(options.iter().map(|option| option.option_id.as_str())) {
            Some(id) => Err(refusal(
                StatisticalErrorCode::ContextSourceDuplicate,
                format!("source {id} is declared twice"),
            )),
            None => Ok(()),
        }
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        budget_tokens(request)?;
        let names = requested(request);
        if let Some(name) = first_duplicate(names.iter().map(String::as_str)) {
            return Err(refusal(
                StatisticalErrorCode::ContextSourceDuplicate,
                format!("source {name} is requested twice"),
            ));
        }
        Ok(())
    }

    fn refine(
        &self,
        request: Option<&DecideRequest>,
        _pinned: &Pinned,
        views: &[CandidateView],
        _policy: &medulla_decision_types::decision::DecisionPolicy,
        policy_digest: &str,
        mut executed: Executed,
    ) -> Result<Executed, String> {
        // A replayed record carries no request and no candidate views: the
        // partition is not re-derived there, the answer stands as stored.
        let Some(request) = request else {
            return Ok(executed);
        };
        let budget = budget_tokens(request)?;
        let mut tokens = Vec::with_capacity(views.len());
        for view in views {
            tokens.push(tokens_of(&view.id, view.numbers.get(TOKENS_KEY).copied())?);
        }
        let (Some(reading), Some(matrix)) = (&executed.reading, &executed.matrix) else {
            // No head read the sources: nothing ranks them, the ladder's
            // abstention stands.
            return Ok(executed);
        };
        let scores = reading.probabilities.as_ref().unwrap_or(&reading.logits);
        let mut sources = Vec::with_capacity(views.len());
        for (view, cost) in views.iter().zip(tokens) {
            let Some(index) = matrix.candidate_ids.iter().position(|id| id == &view.id) else {
                continue;
            };
            let score = scores.get(index).copied().ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    format!("no head score for source {}", view.id),
                )
            })?;
            sources.push(AvailableContextSource {
                source_id: view.id.clone(),
                relevance_q32: q32(score).map_err(|r| r.render())?.value,
                tokens: cost,
                subject: view
                    .texts
                    .get(SUBJECT_KEY)
                    .cloned()
                    .unwrap_or_else(|| view.id.clone()),
                authority_rank: authority_rank(view.texts.get(AUTHORITY_KEY).map(String::as_str)),
            });
        }
        let relevance_min =
            q32(medulla_server_api::knob_config::knobs().context_budget_relevance_min)
                .map_err(|r| r.render())?
                .value;
        let mut result = partition(&sources, budget, relevance_min);
        let visible: BTreeSet<&str> = views.iter().map(|view| view.id.as_str()).collect();
        let mut unreadable: Vec<String> = requested(request)
            .into_iter()
            .filter(|name| !visible.contains(name.as_str()))
            .collect();
        unreadable.sort();
        result.omitted.extend(
            unreadable
                .into_iter()
                .map(|source_id| OmittedContextSource {
                    source_id,
                    reason: OmissionReason::Unauthorized,
                }),
        );
        let advisory = result
            .selected
            .iter()
            .filter_map(|id| sources.iter().find(|source| &source.source_id == id))
            .map(|source| ScoredOption {
                option_id: source.source_id.clone(),
                score: medulla_decision_types::decision::QuantisedValue {
                    scale: medulla_decision_types::decision::QuantScaleTag::Q32,
                    value: source.relevance_q32,
                },
                probability: None,
            })
            .collect::<Vec<_>>();
        executed.ladder.outcome = StatisticalOutcome::Advisory {
            scores: BoundedVec::new(advisory)
                .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?,
            calibrated: false,
        };
        executed.ladder.calibration = None;
        executed.ladder.audit = None;
        executed.ladder.logging = Vec::new();
        executed.context_selection = Some(ContextSourceSelection {
            partition: result,
            policy_digest: policy_digest.to_string(),
            source_digest: digest_text(SNAPSHOT_DOMAIN, &sources),
        });
        Ok(executed)
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .context_selection
            .as_ref()
            .map(premises)
            .unwrap_or_default()
    }
}

/// One `context_source:<id>=<selected or reason>` claim premise per source,
/// and the snapshot digest the partition was decided over.
pub(in crate::decide) fn premises(selection: &ContextSourceSelection) -> Vec<PremiseRef> {
    let claim = |subject: &str, fact: String| PremiseRef {
        subject: subject.to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "candidates".to_string(),
        },
    };
    let mut out = Vec::new();
    for id in &selection.partition.selected {
        out.push(claim(id, format!("context_source:{id}=selected")));
    }
    for omitted in &selection.partition.omitted {
        out.push(claim(
            &omitted.source_id,
            format!(
                "context_source:{}={}",
                omitted.source_id,
                omitted.reason.word()
            ),
        ));
    }
    out.push(claim(
        "context_sources",
        format!("context_source_snapshot:{}", selection.source_digest),
    ));
    out
}
