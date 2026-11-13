//! The `topology` question family (MEDULLA-DECISION-ENGINE-R111.1).
//!
//! The swarm topology of one published template, asked on its own. The
//! family concludes by calling the topology selection of
//! `medulla_compute::assemble` over the template's facts and the schema
//! entailments and capacity headroom the server recorded in the question;
//! the plan, its cap admission and its premises are sealed in the record.
//! Evaluate-only: no capacity lease is taken (acquisition is `DecisionCommit`).

use medulla_compute::assemble::{decide_topology, TopologyConclusion, TopologyQuestionInputs};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::topology::TOPOLOGY_SCHEMA_UNAVAILABLE;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext, TopologyExecuted};
use crate::decide::stat_topology::question_of;

use super::DecisionFamily;

/// The family of the `topology` question kind.
pub struct TopologyFamily;

fn executed(outcome: StatisticalOutcome, topology: Option<TopologyExecuted>) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology,
        thought: None,
    }
}

fn conclude_topology(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let question = question_of(request)?;
    let template = ctx
        .store
        .assembly_template(ctx.tenant_id, &question.template)?;
    let read = question
        .read
        .as_ref()
        .filter(|read| !read.schema_digest.is_empty())
        .ok_or_else(|| {
            format!("{TOPOLOGY_SCHEMA_UNAVAILABLE}: the topology schema digest is missing")
        })?;
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    let conclusion = decide_topology(&TopologyQuestionInputs {
        requirements: &question.requirements,
        read,
        template: &template,
        policy,
        policy_digest: &policy_digest,
    })
    .map_err(|error| crate::decide::assemble::assembly_refusal(&error))?;
    Ok(match conclusion {
        TopologyConclusion::Planned { plan, premises } => executed(
            // A proven plan is not a calibrated score or an authorization grant.
            StatisticalOutcome::Advisory {
                scores: BoundedVec::default(),
                calibrated: false,
            },
            Some(TopologyExecuted { plan, premises }),
        ),
        TopologyConclusion::Abstained(mut reasons) => {
            reasons.truncate(64);
            executed(
                StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(reasons).expect("truncated to the bound"),
                },
                None,
            )
        }
    })
}

impl DecisionFamily for TopologyFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        question_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_topology(ctx, request, policy))
    }
}
