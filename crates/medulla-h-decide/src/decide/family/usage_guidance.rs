//! The `usage_guidance` question family (MEDULLA-DECISION-ENGINE-R177.3 to
//! R177.5).
//!
//! How and when an agent should use a connector for a job class: the steps of
//! the job class in `:precedes` order with the tool of each step, the prompts
//! and the declared cautions. The order is `order_steps`, which breaks ties
//! between steps that may swap by the pooled success rate of their tools.
//! `Decide` is evaluate-only: it writes nothing, and every refusal is typed.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_stats::decision::usage_guidance::{
    order_steps, OrderError, PlanStep, UsageKnobs,
};
use medulla_decision_types::decision::capability_designation::Q32_ONE;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::usage_guidance::{
    UsageGuidance, UsageStep, USAGE_GUIDANCE_CYCLIC_STEPS, USAGE_GUIDANCE_NO_STEPS,
    USAGE_GUIDANCE_UNKNOWN_CONNECTOR,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{BoundedVec, OpaqueId, TenantId};

use crate::decide::stat_designation::{coded, DesignationRuntime};
use crate::decide::stat_executor::{AnsweredFamily, Executed, ExecutionContext};
use crate::decide::stat_support::invalid_parameter;
use crate::decide::stat_usage_guidance::{
    question_of, step_tool, Fact, PackFacts, StepFact, ToolFact, UsageGuidanceRead,
};

use super::capability_designation::executed;
use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R177.4).
pub use medulla_decision_types::decision::usage_guidance::REFUSALS;

/// The family of the `usage_guidance` question kind.
pub struct UsageGuidanceFamily;

fn abstained(code: medulla_types::refusal_code::RefusalCode, detail: String) -> Executed {
    executed(
        StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::Family {
                family: QuestionKind::UsageGuidance,
                code,
                detail,
            }])
            .expect("one reason fits the bound"),
        },
        None,
    )
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A step of the job class with the tool it resolved to.
struct Resolved<'a> {
    step: &'a StepFact,
    tool: &'a ToolFact,
}

/// The pack origin the capability index holds for a tool, when it holds one.
fn origin_of(
    runtime: &DesignationRuntime,
    tenant: &TenantId,
    component_id: &str,
) -> Option<medulla_decision_types::decision::capability_designation::PackOrigin> {
    let id = OpaqueId::new(component_id).ok()?;
    runtime
        .index
        .visible(tenant)
        .iter()
        .find_map(|snapshot| snapshot.record(&id))
        .and_then(|record| record.pack.clone())
}

/// The premise of one supporting triple: a claim of the connector pack.
fn triple_premise(
    runtime: &DesignationRuntime,
    tenant: &TenantId,
    connector: &str,
    tool: Option<&ToolFact>,
    fact: &Fact,
) -> PremiseRef {
    let cite = fact.cite();
    let provenance = match tool.and_then(|tool| origin_of(runtime, tenant, &tool.component_id)) {
        Some(pack) => PremiseProvenance::ConnectorPack {
            connector: pack.connector,
            binding_revision: pack.binding_revision,
            pack_digest: pack.pack_digest,
            entry_digest: pack.entry_digest,
        },
        None => PremiseProvenance::ClaimedMapping {
            text_digest: sha256_hex(&cite),
            producer: connector.to_string(),
        },
    };
    PremiseRef {
        subject: fact.subject.clone(),
        fact: format!("guidance:triple={cite}"),
        class: PremiseClass::Claim,
        provenance,
    }
}

fn resolve<'a>(facts: &'a PackFacts) -> Vec<Resolved<'a>> {
    facts
        .steps
        .iter()
        .filter_map(|step| step_tool(facts, step).map(|tool| Resolved { step, tool }))
        .collect()
}

/// Answer one usage guidance question (MEDULLA-DECISION-ENGINE-R177.3 to R177.5).
pub(crate) fn usage_guidance_outcome(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let question = question_of(request)?;
    let tenant = TenantId::new(request.tenant_id.as_str())
        .map_err(|detail| invalid_parameter(format!("tenant: {detail}")))?;
    let runtime = ctx.designation.as_ref().ok_or_else(|| {
        coded(
            medulla_decision_types::decision::capability_designation::DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
            "the pack facts and tool outcomes were not read",
        )
    })?;
    let read: &UsageGuidanceRead = runtime.usage_guidance.as_ref().ok_or_else(|| {
        coded(
            medulla_decision_types::decision::capability_designation::DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
            "the pack facts of the connector were not read",
        )
    })?;
    let facts = &read.facts;
    if !facts.connector_known {
        return Err(coded(
            USAGE_GUIDANCE_UNKNOWN_CONNECTOR,
            format!("no imported pack declares connector {}", question.connector),
        ));
    }
    let resolved = resolve(facts);
    if resolved.is_empty() {
        return Ok(abstained(
            USAGE_GUIDANCE_NO_STEPS,
            format!(
                "connector {} has no steps with a tool for job class {}",
                question.connector, question.job_class
            ),
        ));
    }
    let by_step: BTreeMap<&str, &Resolved> = resolved
        .iter()
        .map(|entry| (entry.step.step_id.as_str(), entry))
        .collect();
    let plan: Vec<PlanStep> = resolved
        .iter()
        .map(|entry| PlanStep {
            step_id: entry.step.step_id.clone(),
            tool: entry.tool.component_id.clone(),
            precedes: entry.step.precedes.clone(),
        })
        .collect();
    let knobs = UsageKnobs {
        min_trials: medulla_server_api::knob_config::knobs().usage_min_trials,
    };
    let order = order_steps(&plan, &read.rates, &knobs).map_err(|error| match error {
        OrderError::Cyclic(ids) => coded(
            USAGE_GUIDANCE_CYCLIC_STEPS,
            format!(
                "the :precedes edges form a cycle through steps {}",
                ids.join(", ")
            ),
        ),
        OrderError::DuplicateStep(id) => invalid_parameter(format!("step {id} is declared twice")),
    })?;
    // The direct predecessors and successors of each step, as tool ids.
    let tool_of = |step_id: &str| by_step.get(step_id).map(|e| e.tool.component_id.clone());
    let mut steps = Vec::new();
    let mut premises = Vec::new();
    let mut conditions: BTreeSet<String> = BTreeSet::new();
    let mut cautions: Vec<String> = Vec::new();
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut applied_classes: BTreeSet<String> = BTreeSet::new();
    for step_id in &order.steps {
        let entry = by_step[step_id.as_str()];
        let inputs_from: Vec<String> = resolved
            .iter()
            .filter(|other| other.step.precedes.contains(step_id))
            .map(|other| other.tool.component_id.clone())
            .collect();
        let precedes: Vec<String> = entry
            .step
            .precedes
            .iter()
            .filter_map(|next| tool_of(next))
            .collect();
        steps.push(UsageStep {
            tool: entry.tool.component_id.clone(),
            inputs_from,
            precedes,
        });
        for capability in &entry.tool.capabilities {
            conditions.insert(format!("capability:{capability}"));
        }
        for class in &entry.tool.operates_on {
            conditions.insert(format!("operates_on:{class}"));
            applied_classes.insert(class.clone());
        }
        for fact in entry.step.facts.iter() {
            premises.push(triple_premise(
                runtime,
                &tenant,
                &question.connector,
                None,
                fact,
            ));
        }
        for fact in &entry.tool.facts {
            premises.push(triple_premise(
                runtime,
                &tenant,
                &question.connector,
                Some(entry.tool),
                fact,
            ));
        }
        if let Some(skill) = entry
            .step
            .skill
            .as_ref()
            .and_then(|iri| facts.skills.get(iri))
        {
            for fact in &skill.facts {
                premises.push(triple_premise(
                    runtime,
                    &tenant,
                    &question.connector,
                    None,
                    fact,
                ));
            }
            if let Some(text) = &skill.text {
                if declared.insert(text.clone()) {
                    cautions.push(text.clone());
                }
            }
        }
        if let Some(rate) = read.rates.get(&entry.tool.component_id) {
            if rate.trials > 0 {
                premises.push(PremiseRef {
                    subject: entry.tool.component_id.clone(),
                    fact: format!(
                        "guidance:outcomes={}/{}/{}",
                        entry.tool.component_id, rate.successes, rate.trials
                    ),
                    class: PremiseClass::Observation,
                    provenance: PremiseProvenance::Observation {
                        evaluation_id: format!("decision-log@{}", request.tenant_id),
                    },
                });
            }
        }
    }
    for text in &cautions {
        let digest = sha256_hex(text);
        premises.push(PremiseRef {
            subject: question.connector.clone(),
            fact: format!("declared:{digest}"),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::ClaimedMapping {
                text_digest: digest,
                producer: question.connector.clone(),
            },
        });
    }
    let mut seen = BTreeSet::new();
    premises.retain(|premise| seen.insert((premise.subject.clone(), premise.fact.clone())));
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    premises.push(PremiseRef {
        subject: "policy".to_string(),
        fact: format!("knob:usage.min_trials={}", knobs.min_trials),
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Policy { policy_digest },
    });
    let prompts: Vec<String> = facts
        .prompts
        .iter()
        .filter(|prompt| {
            prompt
                .applies_to
                .iter()
                .any(|class| applied_classes.contains(class))
        })
        .map(|prompt| prompt.iri.clone())
        .collect();
    let guidance = UsageGuidance {
        connector: question.connector.clone(),
        job_class: question.job_class.clone(),
        conditions: conditions.into_iter().collect(),
        steps,
        prompts,
        cautions,
        graph_revision: read.graph_revision,
    };
    let count = guidance.steps.len() as i64;
    let mut listed = BTreeSet::new();
    let scores: Vec<ScoredOption> = guidance
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| listed.insert(step.tool.clone()))
        .map(|(position, step)| ScoredOption {
            option_id: step.tool.clone(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: Q32_ONE * (count - position as i64) / count,
            },
            probability: None,
        })
        .collect();
    let (resolution, evidence) = if order.rate_ordered {
        (ResolutionKind::Statistical, EvidenceClass::Observation)
    } else {
        (ResolutionKind::Constraint, EvidenceClass::Claim)
    };
    Ok(executed(
        StatisticalOutcome::Advisory {
            scores: BoundedVec::new(scores).map_err(|detail| invalid_parameter(detail))?,
            calibrated: false,
        },
        Some(AnsweredFamily {
            answer: FamilyAnswer::UsageGuidance(Box::new(guidance)),
            premises,
            resolution: Some(resolution),
            evidence: Some(evidence),
        }),
    ))
}

impl DecisionFamily for UsageGuidanceFamily {
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
        Some(usage_guidance_outcome(ctx, request, policy))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => Some(ResolutionKind::Abstention),
            _ => None,
        }
    }
}
