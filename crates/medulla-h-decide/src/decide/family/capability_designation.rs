//! The `capability_designation` question family (MEDULLA-DECISION-ENGINE-R172.7).
//!
//! Which agents, tools or skills are best able to do a job. The model is
//! `medulla_decision_types::decision::capability_designation`. The family reads
//! the resident index (`medulla_decision_memory`), blends cosine, learned reward
//! and ontology prior (`medulla_decision_rank`), explains every choice with its
//! eligibility, and seals the typed answer in the decision record. `Decide` is
//! evaluate-only: it writes nothing.

use std::collections::{BTreeMap, BTreeSet};

use medulla_agent_types::agent_component::capability_descriptor::ClosureProjection;
use medulla_decision_memory::memory::capability_index::IndexRefusal;
use medulla_decision_rank::rank::capability_rank::{
    rank_capability_candidates, PoolEntry, RankPolicy,
};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::capability_designation::{
    compute_capability_eligibility, CapabilityDesignation, CapabilityReward, DesignatedCapability,
    DesignationAsk, DesignationKnobs, DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
    DESIGNATION_DIM_MISMATCH, DESIGNATION_EMBEDDING_VERSION_MISMATCH,
    DESIGNATION_EMPTY_QUERY_VECTOR, DESIGNATION_INDEX_CAPACITY_EXCEEDED,
    DESIGNATION_MALFORMED_QUERY_VECTOR, DESIGNATION_NO_CAPABLE_CANDIDATE,
    DESIGNATION_NO_DESIGNATED_COMPONENT, DESIGNATION_STALE_PACK_REVISION,
    DESIGNATION_TOO_MANY_CANDIDATES, DESIGNATION_TOO_MANY_REQUIRED_CAPABILITIES,
    DESIGNATION_UNKNOWN_TARGET_CLASS, KNOBS as DESIGNATION_KNOBS,
};
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
use medulla_types::contract::{BoundedVec, OpaqueId, ResourceId};
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_designation::{coded, question_of, DesignationRuntime};
use crate::decide::stat_executor::{AnsweredFamily, Executed, ExecutionContext};
use crate::decide::stat_support::invalid_parameter;

use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R172.8), declared
/// beside the model so the uniqueness test of the code registry collects it.
pub use medulla_decision_types::decision::capability_designation::REFUSALS;

/// The family of the `capability_designation` question kind.
pub struct CapabilityDesignationFamily;

pub(super) fn executed(outcome: StatisticalOutcome, answered: Option<AnsweredFamily>) -> Executed {
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
        topology: None,
        thought: None,
        context_selection: None,
        family_premises: Vec::new(),
        route: None,
        answered,
    }
}

fn abstained_with(code: RefusalCode, detail: String) -> Executed {
    executed(
        StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::Family {
                family: QuestionKind::CapabilityDesignation,
                code,
                detail,
            }])
            .expect("one reason fits the bound"),
        },
        None,
    )
}

/// The registered code of an index refusal.
fn index_code(refusal: &IndexRefusal) -> RefusalCode {
    match refusal {
        IndexRefusal::EmbeddingVersionMismatch { .. } => DESIGNATION_EMBEDDING_VERSION_MISMATCH,
        IndexRefusal::DimMismatch { .. } => DESIGNATION_DIM_MISMATCH,
        IndexRefusal::CapacityExceeded { .. } => DESIGNATION_INDEX_CAPACITY_EXCEEDED,
        IndexRefusal::MalformedVector { .. } | IndexRefusal::InvalidRecord { .. } => {
            DESIGNATION_MALFORMED_QUERY_VECTOR
        }
        IndexRefusal::UnknownEntity { .. } => DESIGNATION_MALFORMED_QUERY_VECTOR,
    }
}

fn index_detail(refusal: &IndexRefusal) -> String {
    match refusal {
        IndexRefusal::EmbeddingVersionMismatch { expected, actual } => {
            format!(
                "the index is pinned to embedding version {expected}, the question offers {actual}"
            )
        }
        IndexRefusal::DimMismatch { expected, actual } => {
            format!("the index is pinned to dimension {expected}, the question offers {actual}")
        }
        IndexRefusal::CapacityExceeded { limit } => {
            format!("a visible shard holds its limit of {limit} records")
        }
        IndexRefusal::MalformedVector { detail } | IndexRefusal::InvalidRecord { detail } => {
            detail.clone()
        }
        IndexRefusal::UnknownEntity { entity_id } => {
            format!("unknown entity {}", entity_id.as_str())
        }
    }
}

/// The query vector as unit `f32`s, or the refusal naming what is wrong.
fn query_of(ask: &DesignationAsk) -> Result<Vec<f32>, String> {
    let vector = &ask.question.query_vector;
    if vector.dim == 0 || vector.f32le_b64.is_empty() {
        return Err(coded(
            DESIGNATION_EMPTY_QUERY_VECTOR,
            "the query vector holds no values",
        ));
    }
    let values = vector
        .decode()
        .map_err(|detail| coded(DESIGNATION_MALFORMED_QUERY_VECTOR, detail))?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(coded(
            DESIGNATION_MALFORMED_QUERY_VECTOR,
            "the query vector holds a value that is not finite",
        ));
    }
    let norm = values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return Err(coded(
            DESIGNATION_EMPTY_QUERY_VECTOR,
            "the query vector has no direction: every value is zero",
        ));
    }
    Ok(values
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect())
}

/// The task embedding after the active adapter, unit length again; the base
/// vector when no adapter is active, the adapter serves another width, or the
/// adapted vector has no direction (MEDULLA-DECISION-ENGINE-R172.13).
pub(crate) fn adapted_query(runtime: &DesignationRuntime, base: Vec<f32>) -> Vec<f32> {
    let Some(applied) = &runtime.adapter else {
        return base;
    };
    let Some(adapted) = applied.served.adapt(&base) else {
        return base;
    };
    let norm = adapted
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return base;
    }
    adapted
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

/// The first required capability no visible record covers, sorted order.
fn first_uncovered(
    runtime: &DesignationRuntime,
    ask: &DesignationAsk,
    subtypes: &BTreeMap<String, BTreeSet<String>>,
) -> Option<String> {
    let snapshots = runtime.index.visible(&ask.question.tenant_id);
    ask.question
        .required_capabilities
        .iter()
        .find(|capability| {
            let one: BTreeSet<String> = BTreeSet::from([(*capability).clone()]);
            snapshots.iter().all(|snapshot| {
                snapshot
                    .filtered_pool(&one, &BTreeSet::new(), subtypes)
                    .is_empty()
            })
        })
        .cloned()
}

fn observation(epoch: u64) -> PremiseProvenance {
    PremiseProvenance::Observation {
        evaluation_id: format!("capability_index@{epoch}"),
    }
}

fn premise(
    subject: &str,
    fact: String,
    class: PremiseClass,
    provenance: PremiseProvenance,
) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact,
        class,
        provenance,
    }
}

fn resource(value: &str) -> Result<ResourceId, String> {
    ResourceId::new(value).map_err(|detail| invalid_parameter(format!("{value}: {detail}")))
}

/// One closure per required capability: the capability and every class below
/// it, sorted, at the schema digest the closure was derived at.
fn closures_of(
    ask: &DesignationAsk,
    question_id: &str,
    subtypes: &BTreeMap<String, BTreeSet<String>>,
    schema_digest: &str,
) -> Result<Vec<ClosureProjection>, String> {
    let request_id = resource(question_id)?;
    ask.question
        .required_capabilities
        .iter()
        .map(|capability| {
            let members = subtypes
                .get(capability)
                .into_iter()
                .flatten()
                .map(|member| resource(member))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ClosureProjection {
                tenant: ask.question.tenant_id.clone(),
                request_id: request_id.clone(),
                capability_id: resource(capability)?,
                members,
                ontology_digest: schema_digest.to_string(),
            })
        })
        .collect()
}

/// The premises of one designation answer: one per designation (entity, cosine,
/// reward, prior, blended score, eligibility), one `knob:<name>=<value>` per
/// knob read, the index epoch, embedding version, schema digest and candidate
/// pool size, and one per subsumption path used (MEDULLA-DECISION-ENGINE-R172.9).
fn premises_of(
    answer: &CapabilityDesignation,
    policy: &DecisionPolicy,
    policy_digest: &str,
) -> Vec<PremiseRef> {
    let epoch = answer.index_epoch;
    let mut out = Vec::new();
    for designated in &answer.designations {
        out.push(premise(
            designated.entity_id.as_str(),
            format!(
                "designation:cosine={};reward={};prior={};score={};eligible={}",
                designated.cosine_q32,
                designated.reward_q32,
                designated.prior_q32,
                designated.score_q32,
                designated.eligibility.eligible
            ),
            PremiseClass::Observation,
            observation(epoch),
        ));
        if !designated.eligibility.target_paths.is_empty() {
            let paths: Vec<String> = designated
                .eligibility
                .target_paths
                .iter()
                .map(|path| path.join(">"))
                .collect();
            let (class, provenance, digest) = match &designated.pack {
                Some(pack) => (
                    PremiseClass::Claim,
                    PremiseProvenance::ConnectorPack {
                        connector: pack.connector.clone(),
                        binding_revision: pack.binding_revision,
                        pack_digest: pack.pack_digest.clone(),
                        entry_digest: pack.entry_digest.clone(),
                    },
                    pack.pack_digest.clone(),
                ),
                None => (PremiseClass::Observation, observation(epoch), String::new()),
            };
            out.push(premise(
                designated.entity_id.as_str(),
                format!(
                    "target_designation:component={};path={};pack_digest={digest};graph_revision={}",
                    designated.entity_id.as_str(),
                    paths.join("|"),
                    designated.record_revision
                ),
                class,
                provenance,
            ));
        }
        for path in &designated.eligibility.subsumption_paths {
            out.push(premise(
                designated.entity_id.as_str(),
                format!("subsumption:{}", path.join(">")),
                PremiseClass::Proof,
                PremiseProvenance::SchemaSource {
                    source_key: "composed".to_string(),
                    schema_digest: answer.schema_digest.clone(),
                },
            ));
        }
    }
    for spec in DESIGNATION_KNOBS {
        out.push(premise(
            "policy",
            format!("knob:{}={}", spec.name, policy.knob(spec).render()),
            PremiseClass::Definition,
            PremiseProvenance::Policy {
                policy_digest: policy_digest.to_string(),
            },
        ));
    }
    for fact in [
        format!("index_epoch={epoch}"),
        format!("embedding_version={}", answer.embedding_version),
        format!("candidate_pool_size={}", answer.candidate_pool_size),
        format!("capability_filtered={}", answer.capability_filtered),
    ] {
        out.push(premise(
            "capability_index",
            fact,
            PremiseClass::Observation,
            observation(epoch),
        ));
    }
    out.push(premise(
        "schema",
        format!("schema_digest={}", answer.schema_digest),
        PremiseClass::Definition,
        PremiseProvenance::SchemaSource {
            source_key: "composed".to_string(),
            schema_digest: answer.schema_digest.clone(),
        },
    ));
    out
}

/// Why a matched record comes from a stale pack revision: some visible record
/// of the same connector carries a newer binding revision
/// (MEDULLA-DECISION-ENGINE-R172.12.4).
fn stale_pack(
    runtime: &DesignationRuntime,
    tenant: &medulla_types::contract::TenantId,
    matched: &[&medulla_decision_memory::memory::capability_index::Hit],
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
    matched.iter().find_map(|hit| {
        let pack = hit.record.pack.as_ref()?;
        let latest = *newest.get(&pack.connector)?;
        (pack.binding_revision < latest).then(|| {
            format!(
                "component {} comes from revision {} of the {} pack, the index holds revision {latest}",
                hit.record.entity_id.as_str(),
                pack.binding_revision,
                pack.connector
            )
        })
    })
}

/// What one designation read concluded: the answer, or an abstention naming
/// its registered code (an abstention is not a request refusal).
pub(crate) enum Designated {
    Answer(CapabilityDesignation),
    Abstain(RefusalCode, String),
}

/// Read one designation question from the resident index
/// (MEDULLA-DECISION-ENGINE-R172.7). `Err` is a typed refusal text; nothing is
/// written. The `load_set` family calls this once per required capability.
pub(crate) fn designate(
    ctx: &ExecutionContext,
    runtime: &DesignationRuntime,
    ask: &DesignationAsk,
    question_id: &str,
    policy: &DecisionPolicy,
) -> Result<Designated, String> {
    let question = &ask.question;
    let knobs = DesignationKnobs::from_policy(policy);
    if question.required_capabilities.len() > knobs.max_required_capabilities {
        return Err(coded(
            DESIGNATION_TOO_MANY_REQUIRED_CAPABILITIES,
            format!(
                "{} required capabilities, at most {} are allowed",
                question.required_capabilities.len(),
                knobs.max_required_capabilities
            ),
        ));
    }
    if usize::from(question.k) > knobs.max_k {
        return Err(coded(
            DESIGNATION_TOO_MANY_CANDIDATES,
            format!("k is {}, at most {} are allowed", question.k, knobs.max_k),
        ));
    }
    question
        .weights
        .validate()
        .map_err(|detail| invalid_parameter(detail))?;
    let query = adapted_query(runtime, query_of(ask)?);
    if !question.required_capabilities.is_empty() || !question.target_classes.is_empty() {
        if let Some(detail) = &runtime.schema_error {
            return Err(coded(DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE, detail));
        }
        if runtime.schema_digest.is_empty() {
            return Err(coded(
                DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
                "the schema closure has no digest",
            ));
        }
    }
    if let Some(unknown) = question
        .target_classes
        .iter()
        .find(|class| !runtime.projection.knows(class))
    {
        return Err(coded(
            DESIGNATION_UNKNOWN_TARGET_CLASS,
            format!("{unknown} is not a class of the composed schema"),
        ));
    }
    // The policy bounds the pool one question reads: a visible shard above
    // the policy's record limit refuses the whole question.
    if let Some(over) = runtime
        .index
        .visible(&question.tenant_id)
        .iter()
        .map(|snapshot| snapshot.live_count())
        .find(|live| *live > knobs.max_records_per_shard)
    {
        return Err(coded(
            DESIGNATION_INDEX_CAPACITY_EXCEEDED,
            format!(
                "a visible shard holds {over} records, above designation.max_records_per_shard {}",
                knobs.max_records_per_shard
            ),
        ));
    }
    let subtypes: BTreeMap<String, BTreeSet<String>> = question
        .required_capabilities
        .iter()
        .map(|capability| {
            (
                capability.clone(),
                runtime.projection.subtypes_of(capability),
            )
        })
        .collect();
    let k = usize::from(question.k);
    let mut shortlist = k.max(knobs.oversample_factor.max(1) * k).max(1);
    if !question.target_classes.is_empty() {
        // A target filter applies before the cut to k: read every live record.
        let widest = runtime
            .index
            .visible(&question.tenant_id)
            .iter()
            .map(|snapshot| snapshot.live_count())
            .max()
            .unwrap_or(0);
        shortlist = shortlist.max(widest);
    }
    let read = runtime
        .index
        .read(
            &question.tenant_id,
            &question.query_vector.embedding_version,
            &query,
            &question.required_capabilities,
            &question.required_policy_tags,
            &subtypes,
            shortlist,
            knobs.flat_threshold,
        )
        .map_err(|refusal| coded(index_code(&refusal), index_detail(&refusal)))?;
    if k > 0 && read.capability_filtered == 0 {
        let first = first_uncovered(runtime, ask, &subtypes).unwrap_or_else(|| {
            "no visible candidate meets every required capability and policy tag".to_string()
        });
        return Ok(Designated::Abstain(DESIGNATION_NO_CAPABLE_CANDIDATE, first));
    }
    let matched: Vec<&medulla_decision_memory::memory::capability_index::Hit> = read
        .hits
        .iter()
        .filter(|hit| {
            question.target_classes.is_empty()
                || !runtime
                    .projection
                    .target_paths(&hit.record, &question.target_classes)
                    .is_empty()
        })
        .collect();
    if !question.target_classes.is_empty() {
        if matched.is_empty() {
            let target = question.target_classes.iter().next().cloned();
            return Ok(Designated::Abstain(
                DESIGNATION_NO_DESIGNATED_COMPONENT,
                format!(
                    "no visible component applies to or executes {}",
                    target.unwrap_or_default()
                ),
            ));
        }
        if let Some(detail) = stale_pack(runtime, &question.tenant_id, &matched) {
            return Err(coded(DESIGNATION_STALE_PACK_REVISION, detail));
        }
    }
    let rewards: BTreeMap<OpaqueId, CapabilityReward> = ctx
        .store
        .capability_rewards(ctx.tenant_id)?
        .into_iter()
        .map(|reward| (reward.entity_id.clone(), reward))
        .collect();
    let by_entity: BTreeMap<OpaqueId, &medulla_decision_memory::memory::capability_index::Hit> =
        matched
            .iter()
            .map(|hit| (hit.record.entity_id.clone(), *hit))
            .collect();
    let pool: Vec<PoolEntry> = matched
        .iter()
        .map(|hit| PoolEntry {
            entity_id: hit.record.entity_id.clone(),
            vector: hit.vector.clone(),
            ontology_type: hit.record.ontology_type.clone(),
            alternatives: hit.record.swappable_with.clone(),
        })
        .collect();
    let ranked = rank_capability_candidates(pool, question, &rewards, &RankPolicy::from(&knobs));
    let designations: Vec<DesignatedCapability> = ranked
        .into_iter()
        .filter_map(|candidate| {
            let hit = by_entity.get(&candidate.entity_id)?;
            let mut eligibility =
                compute_capability_eligibility(&hit.record, question, &runtime.projection);
            eligibility.reward = candidate.reward_q32;
            Some(DesignatedCapability {
                entity_id: candidate.entity_id,
                score_q32: candidate.score_q32,
                cosine_q32: candidate.cosine_q32,
                reward_q32: candidate.reward_q32,
                prior_q32: candidate.prior_q32,
                ontology_type: candidate.ontology_type,
                capabilities: hit.record.capabilities.clone(),
                alternatives: candidate.alternatives,
                applies_to: hit.record.applies_to.clone(),
                executes: hit.record.executes.clone(),
                pack: hit.record.pack.clone(),
                record_revision: hit.record.record_revision,
                eligibility,
            })
        })
        .collect();
    let answer = CapabilityDesignation {
        designations,
        candidate_pool_size: read.candidate_pool_size,
        capability_filtered: read.capability_filtered,
        index_epoch: read.index_epoch,
        embedding_version: read
            .embedding_version
            .clone()
            .unwrap_or_else(|| question.query_vector.embedding_version.clone()),
        schema_digest: runtime.schema_digest.clone(),
        closures: closures_of(ask, question_id, &subtypes, &runtime.schema_digest)?,
    };
    Ok(Designated::Answer(answer))
}

/// Answer one designation question from the resident index
/// (MEDULLA-DECISION-ENGINE-R172.7). Every refusal is typed and writes nothing.
pub(crate) fn capability_designation_outcome(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let ask = question_of(request)?;
    let runtime = ctx.designation.as_ref().ok_or_else(|| {
        coded(
            DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE,
            "the designation index and schema closure were not read",
        )
    })?;
    let answer = match designate(ctx, runtime, ask, &request.question.question_id, policy)? {
        Designated::Answer(answer) => answer,
        Designated::Abstain(code, detail) => return Ok(abstained_with(code, detail)),
    };
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    let mut premises = premises_of(&answer, policy, &policy_digest);
    if let Some(applied) = &runtime.adapter {
        premises.push(premise(
            "adapter",
            applied.fact(),
            PremiseClass::Observation,
            observation(answer.index_epoch),
        ));
    }
    let scores: Vec<ScoredOption> = answer
        .designations
        .iter()
        .map(|designated| ScoredOption {
            option_id: designated.entity_id.as_str().to_string(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: designated.score_q32,
            },
            probability: None,
        })
        .collect();
    Ok(executed(
        StatisticalOutcome::Advisory {
            scores: BoundedVec::new(scores).map_err(|detail| invalid_parameter(detail))?,
            calibrated: false,
        },
        Some(AnsweredFamily {
            answer: FamilyAnswer::CapabilityDesignation(Box::new(answer)),
            premises,
            resolution: None,
            evidence: None,
        }),
    ))
}

impl DecisionFamily for CapabilityDesignationFamily {
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
        Some(capability_designation_outcome(ctx, request, policy))
    }

    /// A designation is a ranking of committed index records, not a claim of
    /// the caller: the index holds no provenance marker, so the weakest class
    /// of its premises (observations and schema definitions) is an observation.
    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => Some(ResolutionKind::Abstention),
            _ => Some(ResolutionKind::Statistical),
        }
    }
}
