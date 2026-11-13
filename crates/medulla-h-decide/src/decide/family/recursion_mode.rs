//! The `recursion_mode` question family (MEDULLA-DECISION-ENGINE-R137.13).
//!
//! Measured signals select whether a run handles its tool output directly or
//! recursively: `mode:recursive` when the output size or the graph size meets
//! its knob threshold. The signals are the caller's measurements, sealed under
//! a digest; both references must resolve in the caller's tenant graph, named by
//! the `graph` param and opened under the same row-level filter as every other
//! graph read. `Decide` is evaluate-only and writes nothing.

use medulla_decision_outcome_types::decision::context_budget::{
    decide_recursion_mode, ContextBudgetSignals, RecursionThresholds, SignalRefusal,
};
use medulla_decision_types::decision::knob_families::context;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::support::{advisory, invalid, single_option, text_param};
use super::DecisionFamily;

/// The family of the `recursion_mode` question kind.
pub struct RecursionModeFamily;

/// The signals param: the JSON text of a `ContextBudgetSignals`.
pub const SIGNALS_PARAM: &str = "signals";
/// The option ids the family may declare: the answer is one of them.
pub const MODE_OPTION: &str = "mode";

fn signals_of(request: &DecideRequest) -> Result<ContextBudgetSignals, String> {
    let Some(text) = text_param(request, SIGNALS_PARAM)? else {
        return Err(invalid(
            SignalRefusal::Invalid {
                detail: "signals are required".to_string(),
            }
            .text(),
        ));
    };
    ContextBudgetSignals::from_json(text).map_err(|refusal| invalid(refusal.text()))
}

/// Both references resolve in the (row-level filtered) tenant graph.
fn resolve_refs(signals: &ContextBudgetSignals, view: &GraphView) -> Result<(), SignalRefusal> {
    for reference in [&signals.tool_call_ref, &signals.graph_snapshot_ref] {
        if view.node_row_object(reference).is_none() {
            return Err(SignalRefusal::CrossTenant {
                reference: reference.clone(),
            });
        }
    }
    Ok(())
}

fn observation(fact: String) -> PremiseRef {
    PremiseRef {
        subject: "recursion_mode".to_string(),
        fact,
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::Request {
            field: "params.signals".to_string(),
        },
    }
}

impl DecisionFamily for RecursionModeFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, MODE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        signals_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let signals = signals_of(request)?;
            let max_age_ms = context::integer(policy, context::SIGNAL_MAX_AGE_MS);
            signals
                .check(ctx.now_ms, max_age_ms)
                .map_err(|refusal| invalid(refusal.text()))?;
            let view = graph_view.ok_or_else(|| {
                invalid("a recursion_mode question reads its references from a named graph")
            })?;
            resolve_refs(&signals, view).map_err(|refusal| invalid(refusal.text()))?;
            let thresholds = RecursionThresholds {
                output_bytes: context::integer(policy, context::RECURSIVE_OUTPUT_BYTES),
                graph_nodes: context::integer(policy, context::RECURSIVE_GRAPH_NODES),
            };
            let (mode, reason) = decide_recursion_mode(&signals, &thresholds);
            advisory(vec![
                mode.option_id().to_string(),
                reason.code().to_string(),
            ])
        })())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let Ok(signals) = signals_of(request) else {
            return Vec::new();
        };
        vec![
            observation(format!(
                "recursion_mode.tool_output_bytes={}",
                signals.tool_output_bytes
            )),
            observation(format!(
                "recursion_mode.graph_node_count={}",
                signals.graph_node_count
            )),
            observation(format!(
                "recursion_mode.inputs_digest={}",
                signals.signal_digest
            )),
        ]
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
