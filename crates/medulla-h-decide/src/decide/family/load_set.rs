//! The `load_set` question family (MEDULLA-DECISION-ENGINE-R176.3).
//!
//! Which tools, skills and agents a job should have loaded. The family asks the
//! capability designation read of `capability_designation` once per required
//! capability and once over the whole job, merges the answers with
//! `merge_load_set` (bounded, ordered, pure), and seals the typed answer with
//! one premise per chosen component. `Decide` is evaluate-only: it writes
//! nothing, and every refusal is typed.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_outcome_types::decision::load_set::{
    merge_load_set, LoadComponent, LoadSetKnobs,
};
use medulla_decision_types::decision::capability_designation::{
    CapabilityDesignationQuestion, DesignatedCapability, DesignationAsk, DesignationKnobs,
    DesignationWeights, EntityKind, PackOrigin, QueryVector,
    DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE, MAX_DESIGNATIONS, Q32_ONE,
};
use medulla_decision_types::decision::load_set::{
    LoadSetAnswer, LoadSetQuestion, LOAD_SET_NO_DESIGNATED_COMPONENT, LOAD_SET_STALE_PACK_REVISION,
};
use medulla_decision_types::decision::load_set_knobs::KNOBS as LOAD_SET_KNOBS;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{BoundedVec, TenantId};

use crate::decide::stat_designation::{coded, DesignationRuntime};
use crate::decide::stat_executor::{AnsweredFamily, Executed, ExecutionContext};
use crate::decide::stat_load_set::question_of;
use crate::decide::stat_support::invalid_parameter;

use super::capability_designation::{designate, executed, Designated};
use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R176.4).
pub use medulla_decision_types::decision::load_set::REFUSALS;

/// The family of the `load_set` question kind.
pub struct LoadSetFamily;

fn tenant_of(request: &DecideRequest) -> Result<TenantId, String> {
    TenantId::new(request.tenant_id.as_str())
        .map_err(|detail| invalid_parameter(format!("tenant: {detail}")))
}

fn abstained(code: medulla_types::refusal_code::RefusalCode, detail: String) -> Executed {
    executed(
        StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::Family {
                family: QuestionKind::LoadSet,
                code,
                detail,
            }])
            .expect("one reason fits the bound"),
        },
        None,
    )
}

/// The designation question one read of the load set asks.
fn ask_of(
    question: &LoadSetQuestion,
    tenant: &TenantId,
    capability: Option<&str>,
    k: usize,
    reward_weight_q32: i64,
) -> Result<DesignationAsk, String> {
    let k = u8::try_from(k.clamp(1, usize::from(MAX_DESIGNATIONS)))
        .map_err(|_| invalid_parameter("k exceeds the designation bound"))?;
    Ok(DesignationAsk {
        question: CapabilityDesignationQuestion {
            tenant_id: tenant.clone(),
            query_vector: QueryVector::from_f32(
                &question.embedding_version,
                &question.job_embedding,
            )
            .map_err(|detail| invalid_parameter(format!("job_embedding: {detail}")))?,
            required_capabilities: capability.into_iter().map(str::to_string).collect(),
            required_policy_tags: BTreeSet::new(),
            k,
            weights: DesignationWeights {
                cosine_q32: Q32_ONE,
                reward_q32: reward_weight_q32,
                prior_q32: 0,
            },
            ontology_prior: None,
            target_classes: BTreeSet::new(),
        },
        graph: question.graph.clone(),
    })
}

/// The merge's view of one designation: the record's kind comes from the
/// resident index, never from a field of the sealed designation layout.
fn components_of(
    runtime: &DesignationRuntime,
    tenant: &TenantId,
    designations: &[DesignatedCapability],
    packs: &mut BTreeMap<String, PackOrigin>,
) -> Vec<LoadComponent> {
    let snapshots = runtime.index.visible(tenant);
    designations
        .iter()
        .map(|designated| {
            let kind = snapshots
                .iter()
                .find_map(|snapshot| snapshot.record(&designated.entity_id))
                .map(|record| record.entity_kind)
                .unwrap_or(EntityKind::Tool);
            if let Some(pack) = &designated.pack {
                packs.insert(designated.entity_id.as_str().to_string(), pack.clone());
            }
            LoadComponent {
                id: designated.entity_id.as_str().to_string(),
                kind,
                score_q32: designated.score_q32,
                capabilities: designated.capabilities.clone(),
                pack: designated
                    .pack
                    .as_ref()
                    .map(|pack| pack.connector.clone())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// A chosen component that comes from a pack revision older than the newest
/// one the index holds for the same connector.
fn stale_pack(
    runtime: &DesignationRuntime,
    tenant: &TenantId,
    chosen: &[LoadComponent],
    packs: &BTreeMap<String, PackOrigin>,
) -> Option<String> {
    let mut newest: BTreeMap<String, u64> = BTreeMap::new();
    for snapshot in runtime.index.visible(tenant) {
        let all = snapshot.filtered_pool(&BTreeSet::new(), &BTreeSet::new(), &BTreeMap::new());
        for slot in all {
            if let Some(pack) = snapshot.record_at(slot).and_then(|r| r.pack.as_ref()) {
                let seen = newest.entry(pack.connector.clone()).or_insert(0);
                *seen = (*seen).max(pack.binding_revision);
            }
        }
    }
    chosen.iter().find_map(|component| {
        let pack = packs.get(&component.id)?;
        let latest = *newest.get(&pack.connector)?;
        (pack.binding_revision < latest).then(|| {
            format!(
                "component {} comes from revision {} of the {} pack, the index holds revision {latest}",
                component.id, pack.binding_revision, pack.connector
            )
        })
    })
}

fn premises_of(
    question: &LoadSetQuestion,
    chosen: &[LoadComponent],
    packs: &BTreeMap<String, PackOrigin>,
    excluded: usize,
    graph_revision: u64,
    policy: &DecisionPolicy,
) -> Vec<PremiseRef> {
    let mut out = Vec::new();
    let required: BTreeSet<&String> = question.required_capabilities.iter().collect();
    for component in chosen {
        let held: Vec<&String> = component
            .capabilities
            .iter()
            .filter(|capability| required.contains(capability))
            .collect();
        let named: Vec<&String> = if held.is_empty() {
            component.capabilities.iter().collect()
        } else {
            held
        };
        let capability = named
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let (class, provenance) = match packs.get(&component.id) {
            Some(pack) => (
                PremiseClass::Claim,
                PremiseProvenance::ConnectorPack {
                    connector: pack.connector.clone(),
                    binding_revision: pack.binding_revision,
                    pack_digest: pack.pack_digest.clone(),
                    entry_digest: pack.entry_digest.clone(),
                },
            ),
            None => (
                PremiseClass::Observation,
                PremiseProvenance::Observation {
                    evaluation_id: format!("graph@{graph_revision}"),
                },
            ),
        };
        out.push(PremiseRef {
            subject: component.id.clone(),
            fact: format!(
                "load_set:component={}/{capability}/{}",
                component.id, component.score_q32
            ),
            class,
            provenance,
        });
    }
    let observation = || PremiseProvenance::Observation {
        evaluation_id: format!("graph@{graph_revision}"),
    };
    out.push(PremiseRef {
        subject: "graph".to_string(),
        fact: format!("load_set:graph_revision={graph_revision}"),
        class: PremiseClass::Observation,
        provenance: observation(),
    });
    out.push(PremiseRef {
        subject: "load_set".to_string(),
        fact: format!("load_set:excluded={excluded}"),
        class: PremiseClass::Observation,
        provenance: observation(),
    });
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    for spec in LOAD_SET_KNOBS {
        out.push(PremiseRef {
            subject: "policy".to_string(),
            fact: format!("knob:{}={}", spec.name, policy.knob(spec).render()),
            class: PremiseClass::Definition,
            provenance: PremiseProvenance::Policy {
                policy_digest: policy_digest.clone(),
            },
        });
    }
    out
}

/// Answer one load-set question (MEDULLA-DECISION-ENGINE-R176.3 to R176.5).
pub(crate) fn load_set_outcome(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let question = question_of(request)?;
    let tenant = tenant_of(request)?;
    let runtime = ctx.designation.as_ref().ok_or_else(|| {
        coded(
            DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
            "the designation index, schema closure and agents were not read",
        )
    })?;
    let read = runtime.load_set.as_ref().ok_or_else(|| {
        coded(
            DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
            "the agents of the load-set graph were not read",
        )
    })?;
    let required: Vec<&String> = {
        let mut seen = BTreeSet::new();
        question
            .required_capabilities
            .iter()
            .filter(|capability| seen.insert(capability.as_str()))
            .collect()
    };
    if question.job_text.trim().is_empty() && required.is_empty() {
        return Err(invalid_parameter(
            "a load set needs job text or a required capability",
        ));
    }
    if required.len() > usize::from(question.max_tools) {
        return Err(invalid_parameter(format!(
            "{} required capabilities exceed max_tools {}",
            required.len(),
            question.max_tools
        )));
    }
    if question.job_embedding.is_empty() || question.embedding_version.is_empty() {
        return Err(invalid_parameter(
            "a load set ranks by the job embedding: job_embedding and embedding_version are required",
        ));
    }
    let knobs = LoadSetKnobs::from_policy(policy);
    let designation_knobs = DesignationKnobs::from_policy(policy);
    let question_id = &request.question.question_id;
    let mut packs: BTreeMap<String, PackOrigin> = BTreeMap::new();
    let mut per_capability: Vec<Vec<LoadComponent>> = Vec::new();
    for capability in &required {
        let ask = ask_of(
            question,
            &tenant,
            Some(capability.as_str()),
            knobs.per_capability_k,
            designation_knobs.reward_weight_q32,
        )?;
        match designate(ctx, runtime, &ask, question_id, policy)? {
            Designated::Answer(answer) => per_capability.push(components_of(
                runtime,
                &tenant,
                &answer.designations,
                &mut packs,
            )),
            Designated::Abstain(_, detail) => {
                // The answer never hides an abstention inside a partial set.
                return Ok(abstained(
                    LOAD_SET_NO_DESIGNATED_COMPONENT,
                    format!("no designated component for capability {capability}: {detail}"),
                ));
            }
        }
    }
    let ask = ask_of(
        question,
        &tenant,
        None,
        usize::from(question.max_tools),
        designation_knobs.reward_weight_q32,
    )?;
    let overall = match designate(ctx, runtime, &ask, question_id, policy)? {
        Designated::Answer(answer) => {
            components_of(runtime, &tenant, &answer.designations, &mut packs)
        }
        Designated::Abstain(_, detail) => {
            if required.is_empty() {
                return Ok(abstained(
                    LOAD_SET_NO_DESIGNATED_COMPONENT,
                    format!("no component is designated for the job: {detail}"),
                ));
            }
            Vec::new()
        }
    };
    let merge = merge_load_set(&per_capability, &overall, &read.agents, question, &knobs);
    if let Some(detail) = stale_pack(runtime, &tenant, &merge.tools, &packs) {
        return Err(coded(LOAD_SET_STALE_PACK_REVISION, detail));
    }
    let premises = premises_of(
        question,
        &merge.tools,
        &packs,
        merge.excluded,
        read.graph_revision,
        policy,
    );
    let scores: Vec<ScoredOption> = merge
        .tools
        .iter()
        .map(|tool| ScoredOption {
            option_id: tool.id.clone(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: tool.score_q32,
            },
            probability: None,
        })
        .collect();
    let answer = LoadSetAnswer {
        tools: merge.tools.iter().map(|tool| tool.id.clone()).collect(),
        skills: merge.skills,
        agents: merge.agents,
        graph_revision: read.graph_revision,
        premises: premises.clone(),
    };
    Ok(executed(
        StatisticalOutcome::Advisory {
            scores: BoundedVec::new(scores).map_err(|detail| invalid_parameter(detail))?,
            calibrated: false,
        },
        Some(AnsweredFamily {
            answer: FamilyAnswer::LoadSet(Box::new(answer)),
            premises,
            resolution: None,
            evidence: None,
        }),
    ))
}

impl DecisionFamily for LoadSetFamily {
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
        Some(load_set_outcome(ctx, request, policy))
    }

    /// Pack facts enter as claims of their connector.
    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => Some(ResolutionKind::Abstention),
            _ => Some(ResolutionKind::Optimization),
        }
    }
}
