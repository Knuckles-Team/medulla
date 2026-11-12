//! The topology question asked on its own (MEDULLA-DECISION-ENGINE-R111.1).
//!
//! `AgentAssemble` poses a topology question to a template inside its agent
//! programme. This asks the same question of one template with no agent
//! variables at all: the same posing rule ([`super::pose_for`]), the same
//! width/rounds/lease/deadline/cap rows ([`super::Question::extend`]), the same
//! solver and the same cap admission, over a programme that holds only the
//! topology choices. The answer is the proven plan and the premises it rests
//! on, or the typed reasons the template cannot answer.

use medulla_agent_types::agent_topology::TopologyInputs;
use medulla_agent_types::agent_topology::TopologyPlan;
use medulla_agent_types::agent_topology::TopologyRequirements;
use medulla_decision_policy::solve::{solve, Model};
use medulla_decision_types::decision::AbstainReason;
use medulla_decision_types::decision::DecisionErrorCode;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_decision_types::decision::PremiseRef;
use medulla_decision_types::decision::TemplateFacts;
use medulla_decision_types::solve::ModelSpec;
use medulla_decision_types::solve::SolveStatus;
use medulla_decision_types::solve::SolverConfig;
use medulla_decision_types::solve::SolverConfigSpec;
use medulla_decision_types::solve::DEFAULT_CERTIFICATE_LEAVES;
use medulla_query_types::solve_scalar::Scalar;
use medulla_types::contract::BoundedVec;

use super::super::model::Built;
use super::super::AssembleError;
use super::{admit, pose_for, Posed};

/// Everything one standalone topology question reads.
pub struct TopologyQuestionInputs<'a> {
    pub requirements: &'a TopologyRequirements,
    /// The schema entailments and capacity headroom, read by the server.
    pub read: &'a TopologyInputs,
    pub template: &'a TemplateFacts,
    pub policy: &'a DecisionPolicy,
    /// Digest of `policy`, recorded beside the caps it supplied.
    pub policy_digest: &'a str,
}

/// What the topology question concluded.
#[derive(Debug, Clone)]
pub enum TopologyConclusion {
    /// The proven plan and the premises it rests on.
    Planned {
        plan: Box<TopologyPlan>,
        premises: Vec<PremiseRef>,
    },
    /// The template cannot answer, and why.
    Abstained(Vec<AbstainReason>),
}

fn infeasible(labels: Vec<String>) -> Vec<AbstainReason> {
    vec![AbstainReason::Infeasible {
        constraints: BoundedVec::new(labels.into_iter().take(64).collect())
            .expect("taken to the bound"),
    }]
}

/// Decide the topology of one template. Pure: the same inputs give the same
/// conclusion.
pub fn decide_topology(
    inputs: &TopologyQuestionInputs<'_>,
) -> Result<TopologyConclusion, AssembleError> {
    let question = match pose_for(
        inputs.policy,
        inputs.requirements,
        Some(inputs.read),
        Some(inputs.template),
    ) {
        Posed::Ask(question) => question,
        Posed::Refused(reasons) => return Ok(TopologyConclusion::Abstained(reasons)),
        Posed::Plain => unreachable!("pose_for always receives the requirements"),
    };
    let mut built = Built {
        spec: ModelSpec {
            variables: Vec::new(),
            constraints: Vec::new(),
            objective: Vec::new(),
        },
        vars: Vec::new(),
        shape: Vec::new(),
    };
    if let Some(reasons) = question.extend(&mut built)? {
        return Ok(TopologyConclusion::Abstained(reasons));
    }
    let config = SolverConfig::try_from(SolverConfigSpec {
        node_budget: inputs.policy.node_budget,
        max_certificate_leaves: DEFAULT_CERTIFICATE_LEAVES,
        bound_denominator: 1,
        accepted_gap: inputs.policy.accepted_gap,
    })
    .map_err(|error| {
        AssembleError::new(DecisionErrorCode::AssemblyInputsInvalid, error.to_string())
    })?;
    let model = Model::try_from(built.spec.clone()).map_err(super::super::search::model_invalid)?;
    let certificate = solve(&model, &config);
    let selected = match &certificate.status {
        SolveStatus::Optimal
        | SolveStatus::OptimalByDeterministicSearch { .. }
        | SolveStatus::FeasibleWithGap { .. } => match &certificate.incumbent {
            Some(incumbent) => incumbent.selected.clone(),
            None => return Ok(TopologyConclusion::Abstained(infeasible(Vec::new()))),
        },
        SolveStatus::Infeasible { core } => {
            let labels = core
                .iter()
                .filter_map(|row| built.spec.constraints.get(row.index()))
                .map(|row| row.label.clone())
                .collect();
            return Ok(TopologyConclusion::Abstained(infeasible(labels)));
        }
        SolveStatus::InfeasibleByDeterministicSearch { .. } => {
            let labels = built
                .spec
                .constraints
                .iter()
                .map(|row| row.label.clone())
                .collect();
            return Ok(TopologyConclusion::Abstained(infeasible(labels)));
        }
        SolveStatus::BudgetExhausted => {
            return Ok(TopologyConclusion::Abstained(vec![
                AbstainReason::BudgetExhausted {
                    incumbent: None,
                    lower_bound: certificate.lower_bound.unwrap_or(Scalar::new(0)),
                },
            ]))
        }
    };
    let plan = question.plan(&built.shape, &selected, 0);
    let record = match admit::admit_topology(&plan, question.facts.depth, &question.caps) {
        Ok(record) => record,
        // A plan that breaches a cap is named by its refusal code, never answered.
        Err(refusal) => {
            return Ok(TopologyConclusion::Abstained(infeasible(vec![refusal
                .code()
                .as_str()
                .to_string()])))
        }
    };
    let mut premises = question.premises();
    premises.extend(admit::premises(&record, inputs.policy_digest));
    Ok(TopologyConclusion::Planned {
        plan: Box::new(plan),
        premises,
    })
}
