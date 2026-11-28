//! The `merge_arming` question family (MEDULLA-SDLC-R019.10).
//!
//! The caller names the delivery, the proposal and the head it sees now. Every other fact is read
//! from the caller's RLS-filtered snapshot of the graph: the delivery record, the landing policy
//! of the branch, the sealed gate verdicts of the head changeset, the owner approval records and
//! the reviews of the proposal. The verdict is [`arming_verdict`] over those facts. A node of
//! another tenant, or one whose seal does not verify, is never a fact. `Decide` is evaluate-only:
//! the engine answers `arm`, `disarm` or `hold` and never merges.

use std::collections::BTreeMap;

use medulla_decision_statistical_types::decision::knobs::KnobValue;
use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_graph_core::graph::GraphView;
use medulla_sdlc::lifecycle::delivery::arming::arming_verdict;
use medulla_source_types::delivery::{
    decode_delivery, ArmingFacts, DELIVERY_RECORD_PROPERTY, DELIVERY_TYPE,
};
use medulla_source_types::gate_verdict::{
    gate_verdict_seal, landing_gate_check, landing_policy_node_id, GateVerdictRecord,
    LandingPolicy, GATE_VERDICT_RECORD_PROPERTY, GATE_VERDICT_SEAL_PROPERTY, GATE_VERDICT_TYPE,
    LANDING_POLICY_RECORD_PROPERTY, LANDING_POLICY_TYPE,
};
use medulla_types::evidence_class::EvidenceClass;
use medulla_work_types::step_verdict::VerificationVerdict;
use {
    medulla_decision_policy_types::decision::merge_arming::option_id,
    medulla_decision_policy_types::decision::merge_arming::MergeArmingRefusal,
    medulla_decision_policy_types::decision::merge_arming::KNOB_ARM_MAX_CHANGED_LINES,
    medulla_decision_policy_types::decision::merge_arming::OWNER_APPROVAL_TYPE,
    medulla_decision_policy_types::decision::merge_arming::PARAM_BRANCH,
    medulla_decision_policy_types::decision::merge_arming::PARAM_CHANGED_LINES,
    medulla_decision_policy_types::decision::merge_arming::PARAM_DELIVERY,
    medulla_decision_policy_types::decision::merge_arming::PARAM_GRAPH,
    medulla_decision_policy_types::decision::merge_arming::PARAM_HEAD_CHANGESET,
    medulla_decision_policy_types::decision::merge_arming::PARAM_HEAD_COMMIT,
    medulla_decision_policy_types::decision::merge_arming::PARAM_PROPOSAL,
    medulla_decision_policy_types::decision::merge_arming::PARAM_REQUIRED_CHECK,
    medulla_decision_policy_types::decision::merge_arming::REVIEW_TYPE,
    medulla_decision_policy_types::decision::merge_arming::ROW_AT_MS,
    medulla_decision_policy_types::decision::merge_arming::ROW_HEAD_COMMIT,
    medulla_decision_policy_types::decision::merge_arming::ROW_PROPOSAL,
    medulla_decision_policy_types::decision::merge_arming::ROW_REVIEWER,
    medulla_decision_policy_types::decision::merge_arming::ROW_REVIEW_STATE,
    medulla_decision_policy_types::decision::merge_arming::STATE_CHANGES_REQUESTED,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};

use medulla_h_decide_base::decide::stat_support::filtered_view;
use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::abstained,
    medulla_h_decide_base::decide::family::support::advisory,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::single_option,
    medulla_h_decide_base::decide::family::support::text_param,
    medulla_h_decide_base::decide::family::support::unsigned_param,
};

/// The one option a merge-arming question declares.
pub const ARMING_OPTION: &str = "merge_arming";

/// The family of the `merge_arming` question kind.
pub struct MergeArmingFamily;

type Row = serde_json::Map<String, serde_json::Value>;

fn required<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a merge_arming question requires {name}")))
}

/// Open the graph holding the delivery before any read, through the caller's row-level filter.
pub async fn arming_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    filtered_view(state, agent_id, required(request, PARAM_GRAPH)?).await
}

fn text(row: &Row, key: &str) -> Option<String> {
    row.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn visible(row: &Row, tenant: &str) -> bool {
    row.get("tenant_id")
        .and_then(|v| v.as_str())
        .is_none_or(|owner| owner == tenant)
}

/// Every visible node row of `kind` that concerns `proposal`, with its id.
fn rows_of(view: &GraphView, tenant: &str, kind: &str, proposal: &str) -> Vec<(String, Row)> {
    let mut rows: Vec<(String, Row)> = view
        .node_properties
        .keys()
        .filter_map(|id| {
            let row = view.node_row_object(id)?;
            (text(&row, "type").as_deref() == Some(kind)
                && visible(&row, tenant)
                && text(&row, ROW_PROPOSAL).as_deref() == Some(proposal))
            .then(|| (id.clone(), row))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn recorded_at(row: &Row) -> u64 {
    row.get(ROW_AT_MS).and_then(|v| v.as_u64()).unwrap_or(0)
}

/// The gate verdicts of `changeset` whose stored seal equals the seal of their record.
fn sealed_verdicts(
    view: &GraphView,
    tenant: &str,
    changeset: &str,
) -> Vec<(String, VerificationVerdict)> {
    view.node_properties
        .keys()
        .filter_map(|id| {
            let row = view.node_row_object(id)?;
            if text(&row, "type").as_deref() != Some(GATE_VERDICT_TYPE) || !visible(&row, tenant) {
                return None;
            }
            let record: GateVerdictRecord =
                serde_json::from_str(&text(&row, GATE_VERDICT_RECORD_PROPERTY)?).ok()?;
            let seal = text(&row, GATE_VERDICT_SEAL_PROPERTY)?;
            (gate_verdict_seal(&record) == seal && record.changeset == changeset)
                .then_some((record.gate_id, record.verdict))
        })
        .collect()
}

fn premise(fact: String, delivery: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "merge_arming".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(delivery.to_string()),
    }
}

/// The facts of one question, read from the view.
pub(in crate::decide) fn facts_of(
    request: &DecideRequest,
    tenant: &str,
    policy: &DecisionPolicy,
    view: &GraphView,
) -> Result<ArmingFacts, String> {
    let delivery_id = required(request, PARAM_DELIVERY)?;
    let proposal = required(request, PARAM_PROPOSAL)?;
    let head_commit = required(request, PARAM_HEAD_COMMIT)?;
    let head_changeset = required(request, PARAM_HEAD_CHANGESET)?;
    let branch = required(request, PARAM_BRANCH)?;
    let required_check = required(request, PARAM_REQUIRED_CHECK)?;
    let changed_lines = unsigned_param(request, PARAM_CHANGED_LINES)?
        .ok_or_else(|| invalid("a merge_arming question requires changed_lines"))?;

    // The delivery: absent and another tenant's read the same, so nothing is disclosed.
    let not_visible = || {
        invalid(format!(
            "the delivery {delivery_id} is not visible to the caller"
        ))
    };
    let row = view
        .node_row_object(delivery_id)
        .filter(|row| text(row, "type").as_deref() == Some(DELIVERY_TYPE))
        .ok_or_else(not_visible)?;
    let delivery = text(&row, DELIVERY_RECORD_PROPERTY)
        .and_then(|record| serde_json::from_str(&record).ok())
        .and_then(|value| decode_delivery(value).ok())
        .filter(|delivery| delivery.tenant == tenant)
        .ok_or_else(not_visible)?;
    let delivery_opened_proposal = delivery.proposals.iter().any(|p| p.id == proposal);

    // The gates: a branch with no landing policy establishes nothing.
    let policy_row = view
        .node_row_object(&landing_policy_node_id(&delivery.repo, branch))
        .filter(|row| {
            text(row, "type").as_deref() == Some(LANDING_POLICY_TYPE) && visible(row, tenant)
        });
    let landing: Option<LandingPolicy> = policy_row
        .and_then(|row| text(&row, LANDING_POLICY_RECORD_PROPERTY))
        .and_then(|record| serde_json::from_str(&record).ok());
    let verdicts = sealed_verdicts(view, tenant, head_changeset);
    let gates_passed = landing
        .as_ref()
        .is_some_and(|landing| landing_gate_check(landing, &verdicts).is_ok());
    let check_passed = verdicts
        .iter()
        .any(|(gate, verdict)| gate == required_check && *verdict == VerificationVerdict::Pass);

    // The owner approval: one that names the head wins, else the latest names the commit it saw.
    let approvals = rows_of(view, tenant, OWNER_APPROVAL_TYPE, proposal);
    let approval_commit = approvals
        .iter()
        .find(|(_, row)| text(row, ROW_HEAD_COMMIT).as_deref() == Some(head_commit))
        .and_then(|(_, row)| text(row, ROW_HEAD_COMMIT))
        .or_else(|| {
            approvals
                .iter()
                .max_by_key(|(id, row)| (recorded_at(row), id.clone()))
                .and_then(|(_, row)| text(row, ROW_HEAD_COMMIT))
        });

    // The reviews: the head the latest saw, and whether any reviewer's latest asks for changes.
    let reviews = rows_of(view, tenant, REVIEW_TYPE, proposal);
    let reviewed_head = reviews
        .iter()
        .max_by_key(|(id, row)| (recorded_at(row), id.clone()))
        .and_then(|(_, row)| text(row, ROW_HEAD_COMMIT))
        .unwrap_or_else(|| head_commit.to_string());
    let mut latest: BTreeMap<String, (u64, String, bool)> = BTreeMap::new();
    for (id, row) in &reviews {
        let reviewer = text(row, ROW_REVIEWER).unwrap_or_else(|| id.clone());
        let key = (recorded_at(row), id.clone());
        let asks = text(row, ROW_REVIEW_STATE).as_deref() == Some(STATE_CHANGES_REQUESTED);
        match latest.get(&reviewer) {
            Some((at, held, _)) if (*at, held.clone()) >= key => {}
            _ => {
                latest.insert(reviewer, (key.0, key.1, asks));
            }
        }
    }
    let changes_requested = latest.values().any(|(_, _, asks)| *asks);

    let max_changed_lines = match policy.knob(
        &medulla_decision_program_types::decision::lifecycle::KNOBS[KNOB_ARM_MAX_CHANGED_LINES],
    ) {
        KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
        _ => 0,
    };
    Ok(ArmingFacts {
        delivery_opened_proposal,
        gates_passed,
        check_passed,
        head_commit: head_commit.to_string(),
        reviewed_head,
        approval_commit,
        changes_requested,
        changed_lines,
        max_changed_lines,
    })
}

fn conclude_over(
    request: &DecideRequest,
    tenant: &str,
    policy: &DecisionPolicy,
    view: Option<&GraphView>,
) -> Result<Executed, String> {
    let view = view.ok_or_else(|| invalid("a merge_arming question reads a graph"))?;
    let facts = facts_of(request, tenant, policy, view)?;
    let delivery = required(request, PARAM_DELIVERY)?;
    match arming_verdict(&facts) {
        Err(refusal) => Ok(abstained(
            QuestionKind::MergeArming,
            &MergeArmingRefusal(refusal),
        )),
        Ok(verdict) => {
            let mut executed = advisory(vec![option_id(verdict).to_string()])?;
            executed.family_premises = vec![
                premise(
                    format!("merge_arming.verdict={}", option_id(verdict)),
                    delivery,
                ),
                premise(
                    format!("merge_arming.gates_passed={}", facts.gates_passed),
                    delivery,
                ),
                premise(
                    format!("merge_arming.check_passed={}", facts.check_passed),
                    delivery,
                ),
                premise(
                    format!("merge_arming.head_commit={}", facts.head_commit),
                    delivery,
                ),
                premise(
                    format!("merge_arming.changed_lines={}", facts.changed_lines),
                    delivery,
                ),
            ];
            Ok(executed)
        }
    }
}

impl DecisionFamily for MergeArmingFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ARMING_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        for name in [
            PARAM_GRAPH,
            PARAM_DELIVERY,
            PARAM_PROPOSAL,
            PARAM_HEAD_COMMIT,
            PARAM_HEAD_CHANGESET,
            PARAM_BRANCH,
            PARAM_REQUIRED_CHECK,
        ] {
            required(request, name)?;
        }
        unsigned_param(request, PARAM_CHANGED_LINES)?
            .ok_or_else(|| invalid("a merge_arming question requires changed_lines"))?;
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_over(request, ctx.tenant_id, policy, graph_view))
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

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
