//! The `skill_evolution` question family (MEDULLA-DECISION-ENGINE-R703.3).
//!
//! Which skills the library should gain, rename or merge, from what runs asked
//! for and used. The candidates are the tenant's visible skills (an
//! `AgentLibrary` scope of `AgentComponentKind::Skill`); the caller passes the
//! run-trace facts as two IRI lists:
//!
//! * `requested_skills`: a requested id no visible skill carries is a gap;
//! * `used_terms`: a term the capability IRI resolver rejects
//!   (`AbstainReason::UnresolvedCapabilityIri`) and that runs used at least
//!   `skill.neologism_min_uses` times is a neologism;
//! * merges: `merge_candidates` over the visible skills, with stored
//!   embeddings the engine reads through the caller's graph access
//!   (`with_skill_embeddings`) from the graph named by `skill_embedding_graph`.
//!
//! The answer is `Advisory` (options `gap:<id>`, `neologism:<term>`,
//! `merge:<a>+<b>`) and writes nothing; publishing a change uses the guarded
//! promotion.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use medulla_agent_types::agent_ontology::{native_under, CAPABILITY_ROOT};
use medulla_decision_stats::decision::skill_evolution::{
    merge_candidates, MergeParams, SkillEntry,
};
use medulla_decision_types::decision::knob_families::skill::{integer, NEOLOGISM_MIN_USES};
use medulla_decision_types::decision::skill_evolution::{
    SkillGap, SkillMergeCandidate, SkillNeologism, PARAM_EMBEDDING_GRAPH, PARAM_REQUESTED_SKILLS,
    PARAM_SKILL_EMBEDDINGS, PARAM_USED_TERMS,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use tokio::sync::RwLock;

use crate::decide::candidates::library_candidates;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_vectors::graph_vectors;

use super::support::{advisory_scored, invalid, iri_list_param, param, text_param};
use super::DecisionFamily;

/// The Q32 score of a proposal that needs no measure: a gap or a neologism.
const FULL_SCORE: i64 = 1i64 << 32;
/// Parts per million in one unit.
const PPM: i64 = 1_000_000;
/// Fixed-point scale of a stored embedding component handed to the detector.
const EMBEDDING_SCALE: f64 = 1_000_000.0;

/// The family of the `skill_evolution` question kind.
pub struct SkillEvolutionFamily;

impl DecisionFamily for SkillEvolutionFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        match &request.candidates {
            CandidateSource::AgentLibrary { scope }
                if scope.kinds.as_slice() == [AgentComponentKind::Skill] =>
            {
                Ok(())
            }
            _ => Err(invalid(
                "a skill_evolution question reads the visible skills: an agent library scope of kind skill",
            )),
        }
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(evolve(ctx, request, policy))
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
}

/// The capability IRI resolver's verdict on a term: the abstention reason it
/// raises when the term is not a native capability term.
fn resolver_rejects(term: &str) -> Option<AbstainReason> {
    (!native_under(term, CAPABILITY_ROOT)).then(|| AbstainReason::UnresolvedCapabilityIri {
        iri: term.to_string(),
    })
}

/// How many times each entry of `list` occurs, in id order.
fn tally(list: &[String]) -> BTreeMap<&str, u32> {
    let mut counts = BTreeMap::new();
    for item in list {
        let slot = counts.entry(item.as_str()).or_insert(0u32);
        *slot = slot.saturating_add(1);
    }
    counts
}

/// Requested skills no visible skill carries.
fn gaps(requested: &[String], visible: &BTreeSet<&str>) -> Vec<SkillGap> {
    tally(requested)
        .into_iter()
        .filter(|(id, _)| !visible.contains(id))
        .map(|(id, runs)| SkillGap {
            skill_id: id.to_string(),
            requested_by_runs: runs,
        })
        .collect()
}

/// Terms the resolver rejects that runs used at least `min_uses` times.
fn neologisms(used: &[String], min_uses: u64) -> Vec<SkillNeologism> {
    tally(used)
        .into_iter()
        .filter(|(term, uses)| u64::from(*uses) >= min_uses && resolver_rejects(term).is_some())
        .map(|(term, uses)| SkillNeologism {
            term: term.to_string(),
            uses,
        })
        .collect()
}

/// The skill as the merge detector sees it.
fn skill_entry(entry: &AgentComponentEntry, embeddings: &BTreeMap<String, Vec<i32>>) -> SkillEntry {
    SkillEntry {
        skill_id: entry.component_id.clone(),
        side_effect_class: entry.side_effect_class(),
        capability_ids: entry.declared_capabilities.iter().cloned().collect(),
        embedding: embeddings
            .get(&entry.component_id)
            .cloned()
            .unwrap_or_default(),
    }
}

/// The Q32 score of an overlap in parts per million.
fn overlap_q32(overlap_ppm: u32) -> i64 {
    i64::from(overlap_ppm) * FULL_SCORE / PPM
}

fn evolve(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let CandidateSource::AgentLibrary { scope } = &request.candidates else {
        return Err(invalid(
            "a skill_evolution question reads an agent library scope",
        ));
    };
    let requested = iri_list_param(request, PARAM_REQUESTED_SKILLS)?;
    let used = iri_list_param(request, PARAM_USED_TERMS)?;
    let embeddings: BTreeMap<String, Vec<i32>> = match param(request, PARAM_SKILL_EMBEDDINGS)? {
        None => BTreeMap::new(),
        Some(TypedValue::Text(text)) => serde_json::from_str(text)
            .map_err(|e| invalid(format!("parameter {PARAM_SKILL_EMBEDDINGS}: {e}")))?,
        Some(_) => {
            return Err(invalid(format!(
                "parameter {PARAM_SKILL_EMBEDDINGS} is not JSON text"
            )))
        }
    };
    let read = library_candidates(ctx.store, ctx.tenant_id, scope)?;
    let visible: BTreeSet<&str> = read
        .entries
        .iter()
        .map(|e| e.component_id.as_str())
        .collect();
    let skills: Vec<SkillEntry> = read
        .entries
        .iter()
        .map(|entry| skill_entry(entry, &embeddings))
        .collect();
    let gaps = gaps(requested, &visible);
    let neologisms = neologisms(used, integer(policy, &NEOLOGISM_MIN_USES));
    let merges: Vec<SkillMergeCandidate> =
        merge_candidates(&skills, &MergeParams::from_policy(policy)).candidates;

    let mut options: Vec<(String, i64)> = Vec::new();
    let mut premises: Vec<FamilyPremise> = Vec::new();
    for gap in &gaps {
        options.push((format!("gap:{}", gap.skill_id), FULL_SCORE));
        premises.push(FamilyPremise {
            subject: "skill_evolution",
            fact: format!("gap:{}:{}", gap.skill_id, gap.requested_by_runs),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request(PARAM_REQUESTED_SKILLS),
        });
    }
    for neologism in &neologisms {
        options.push((format!("neologism:{}", neologism.term), FULL_SCORE));
        let iri = match resolver_rejects(&neologism.term) {
            Some(AbstainReason::UnresolvedCapabilityIri { iri }) => iri,
            _ => neologism.term.clone(),
        };
        premises.push(FamilyPremise {
            subject: "skill_evolution",
            fact: format!("unresolved_capability_iri:{iri}:{}", neologism.uses),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request(PARAM_USED_TERMS),
        });
    }
    for merge in &merges {
        options.push((
            format!("merge:{}+{}", merge.a, merge.b),
            overlap_q32(merge.overlap_ppm),
        ));
        for id in [&merge.a, &merge.b] {
            if let Some(entry) = read.entries.iter().find(|e| &e.component_id == id) {
                premises.push(FamilyPremise {
                    subject: "skill_evolution",
                    fact: format!(
                        "merge:{}+{}:overlap={}:cosine={}",
                        merge.a, merge.b, merge.overlap_ppm, merge.cosine_ppm
                    ),
                    class: PremiseClass::Observation,
                    source: FamilyPremiseSource::Publisher {
                        component_id: entry.component_id.clone(),
                        definition_digest: entry.content_digest.clone(),
                    },
                });
            }
        }
    }
    let mut executed = advisory_scored(options)?;
    executed.family_premises = premises;
    Ok(executed)
}

/// Read the stored embedding of every visible skill from the graph named by
/// `skill_embedding_graph`, through the caller's graph access, and record the
/// vectors in `skill_embeddings` before the family concludes. The parameter is
/// engine state: a request that already carries it is refused. A request that
/// names no graph proposes no merge (a skill without an embedding never
/// qualifies).
pub(in crate::decide) async fn with_skill_embeddings(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    if param(&request, PARAM_SKILL_EMBEDDINGS)?.is_some() {
        return Err(invalid(format!(
            "parameter {PARAM_SKILL_EMBEDDINGS} is written by the engine, not the request"
        )));
    }
    let CandidateSource::AgentLibrary { scope } = &request.candidates else {
        return Err(invalid(
            "a skill_evolution question reads an agent library scope",
        ));
    };
    let Some(graph) = text_param(&request, PARAM_EMBEDDING_GRAPH)?
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
    else {
        return Ok(request);
    };
    let read = library_candidates(store, &request.tenant_id, scope)?;
    let vectors = graph_vectors(state, verified.agent_id(), &graph).await?;
    let mut embeddings: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for entry in &read.entries {
        let Some(vector) = vectors.stored_vector(&entry.component_id) else {
            continue;
        };
        embeddings.insert(
            entry.component_id.clone(),
            vector
                .into_iter()
                .map(|x| {
                    (f64::from(x) * EMBEDDING_SCALE)
                        .round()
                        .clamp(-2.0e9, 2.0e9) as i32
                })
                .collect(),
        );
    }
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: PARAM_SKILL_EMBEDDINGS.to_string(),
        value: TypedValue::Text(serde_json::to_string(&embeddings).map_err(invalid)?),
    });
    params.sort_by(|a, b| a.name.cmp(&b.name));
    request.params = BoundedVec::new(params).map_err(invalid)?;
    Ok(request)
}
