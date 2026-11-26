//! The `tool_action_admission` question family (MEDULLA-DECISION-ENGINE-R038.2).
//!
//! The harness asks whether an action of a listed class may follow content it
//! read. The content is a node of the graph the question names, opened through
//! the caller's access check and row-level filter; its recorded `trust_mark`
//! decides: an action following `untrusted` content is denied, any other
//! answer is `admit`. A mark that is missing from the caller's view abstains.
//! The verdict is advisory: it never grants a human approval, and `Decide`
//! writes nothing.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::tool_action::{
    ActionClass, ToolActionAnswer, ToolActionAnswerVerdict, ToolActionRefusal, ToolActionVerdict,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::filtered_view;

use super::support::{abstained, advisory, invalid, single_option, text_param};
use super::DecisionFamily;

/// The one option a tool-action admission question declares.
pub const ADMISSION_OPTION: &str = "admission";

/// The family of the `tool_action_admission` question kind.
pub struct ToolActionAdmissionFamily;

/// The text parameter `name`, required and non-empty.
fn required<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a tool_action_admission question requires {name}")))
}

/// Open the graph holding the referenced content before any read, through the
/// caller's row-level filter.
pub(in crate::decide) async fn admission_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    filtered_view(state, agent_id, required(request, "graph")?).await
}

impl DecisionFamily for ToolActionAdmissionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMISSION_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        for name in [
            "graph",
            "pending_call_id",
            "params_digest",
            "trust_mark_ref",
        ] {
            required(request, name)?;
        }
        let class = required(request, "action_class")?;
        ActionClass::parse(class).map(|_| ()).ok_or_else(|| {
            invalid(format!(
                "ACTION_CLASS_UNKNOWN: `{class}` is not egress, privileged_call or delegation"
            ))
        })
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(admit(request, graph_view))
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

    fn evidence_class(
        &self,
        outcome: &medulla_decision_types::decision::statistical::StatisticalOutcome,
    ) -> Option<medulla_types::evidence_class::EvidenceClass> {
        use medulla_decision_types::decision::statistical::StatisticalOutcome;
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(medulla_types::evidence_class::EvidenceClass::Observation)
    }
}

fn admit(request: &DecideRequest, view: Option<&GraphView>) -> Result<Executed, String> {
    let content_ref = required(request, "trust_mark_ref")?;
    let view = view.ok_or_else(|| invalid("a tool_action_admission question reads a graph"))?;
    let mark = view.node_row_object(content_ref).and_then(|props| {
        props
            .get("trust_mark")
            .and_then(|v| v.as_str().map(str::to_string))
    });
    // The pending call the verdict governs; the decision ref is the sealing
    // record's id, filled in when the record is sealed.
    let approval_ref = required(request, "pending_call_id")?.to_string();
    let Some(verdict) = mark.as_deref().and_then(ToolActionVerdict::for_mark) else {
        let refusal = ToolActionRefusal::TrustMarkMissing {
            content_ref: content_ref.to_string(),
        };
        let mut executed = abstained(QuestionKind::ToolActionAdmission, &refusal);
        executed.family = Some(answer_of(ToolActionAnswer {
            verdict: ToolActionAnswerVerdict::Abstain,
            approval_ref,
            decision_ref: String::new(),
            reason: Some(refusal.code().as_str().to_string()),
        }));
        return Ok(executed);
    };
    let mut executed = advisory(vec![verdict.option_id().to_string()])?;
    executed.family = Some(answer_of(ToolActionAnswer {
        verdict: match verdict {
            ToolActionVerdict::Admit => ToolActionAnswerVerdict::Admit,
            ToolActionVerdict::Deny => ToolActionAnswerVerdict::Deny,
        },
        approval_ref,
        decision_ref: String::new(),
        reason: None,
    }));
    executed.family_premises = vec![FamilyPremise {
        subject: "tool_action".into(),
        fact: format!("trust_mark:{content_ref}"),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(content_ref.to_string()),
    }];
    Ok(executed)
}

/// The typed answer a question seals; the decision ref is set with the record.
fn answer_of(answer: ToolActionAnswer) -> FamilyExecuted {
    FamilyExecuted {
        answer: Some(FamilyAnswer::ToolActionAdmission(Box::new(answer))),
        premises: Vec::new(),
        resolution: None,
        evidence: None,
    }
}
