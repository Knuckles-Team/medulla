//! The `tool_relevance` question family (MEDULLA-DECISION-ENGINE-R134.110.3).
//!
//! Which tools a role should see for one task. The model is
//! `medulla_decision_types::decision::tool_relevance`. The family reads the
//! tenant's [`ToolIndex`](crate::decide::tool_index::ToolIndex) (R134.110.7),
//! fuses the lexical and embedding arms (`fuse_tool_ranks`, R134.110.2),
//! ranks and selects a covering subset (`rank_tools_by_relevance`, R134.2) and
//! seals an advisory answer with one scored option per selected tool, in rank
//! order. Visibility and tenant filtering happen before any score: the index
//! holds only the tenant's published tools the library serves to it. A catalog
//! change reaches the ranking through the library generation the index
//! compares; there is no second catalog hook. Nothing is written.

use medulla_decision_stats::decision::tool_relevance::{fuse_tool_ranks, rank_tools_by_relevance};
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::tool_relevance::{
    max_tools_from_policy, rrf_k_from_policy, ToolAssignmentRequest, ToolRankingKnobs,
    ToolRelevanceRefusal, CATALOG_TOO_LARGE, UNKNOWN_ROLE,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use std::collections::BTreeMap;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::tool_index::{ToolIndexCache, ToolIndexError};

use super::support::{abstained, advisory, invalid};
use super::DecisionFamily;

/// The parameter carrying the typed assignment request.
pub const PARAM_ASSIGNMENT: &str = "tool_assignment";

/// The family of the `tool_relevance` question kind.
pub struct ToolRelevanceFamily;

fn assignment(request: &DecideRequest) -> Result<&ToolAssignmentRequest, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == PARAM_ASSIGNMENT);
    match (found.next(), found.next()) {
        (Some(param), None) => match &param.value {
            TypedValue::ToolAssignment(assignment) => Ok(assignment.as_ref()),
            _ => Err(invalid(format!(
                "parameter {PARAM_ASSIGNMENT} is not a tool_assignment"
            ))),
        },
        _ => Err(invalid(format!(
            "a tool_relevance question requires exactly one {PARAM_ASSIGNMENT} parameter"
        ))),
    }
}

fn claim(fact: String, field: &'static str) -> FamilyPremise {
    FamilyPremise {
        subject: "tool_relevance",
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(field),
    }
}

fn conclude(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let asked = assignment(request)?;
    asked
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let max_tools = max_tools_from_policy(policy);
    let index = ToolIndexCache::of(ctx.store)
        .get(ctx.store, ctx.tenant_id, max_tools)
        .map_err(|error| match error {
            ToolIndexError::TooLarge { visible, max } => invalid(format!(
                "{}: {visible} visible tools exceed tool_subset.max_tools {max}",
                CATALOG_TOO_LARGE.as_str()
            )),
            ToolIndexError::Read(detail) => invalid(detail),
        })?;
    let kind = QuestionKind::ToolRelevance;
    if index.tools.is_empty() {
        return Ok(abstained(kind, &ToolRelevanceRefusal::EmptyCatalog));
    }
    let allowlist: Option<&[String]> = if asked.agent_role_iri.is_empty() {
        None
    } else {
        match index.roles.get(&asked.agent_role_iri) {
            Some(allowed) => Some(allowed.as_slice()),
            None => {
                return Err(invalid(format!(
                    "{}: {} names no published toolset",
                    UNKNOWN_ROLE.as_str(),
                    asked.agent_role_iri
                )))
            }
        }
    };
    let fused = fuse_tool_ranks(&index.tools, asked, allowlist, rrf_k_from_policy(policy))
        .map_err(|error| invalid(error.to_string()))?;
    if fused.is_empty() {
        return Ok(abstained(kind, &ToolRelevanceRefusal::EmptyCatalog));
    }
    let by_id: BTreeMap<&str, &medulla_decision_stats::decision::tool_relevance::IndexedTool> =
        index
            .tools
            .iter()
            .map(|tool| (tool.entry.tool_id.as_str(), tool))
            .collect();
    let entries: Vec<_> = fused
        .iter()
        .filter_map(|assigned| by_id.get(assigned.tool_id.as_str()))
        .map(|tool| tool.entry.clone())
        .collect();
    let cheapest = entries.iter().map(|entry| entry.token_cost).min();
    if cheapest.is_some_and(|cost| asked.context_budget_tokens < cost) {
        return Ok(abstained(kind, &ToolRelevanceRefusal::BudgetBelowMinimum));
    }
    let relevance: BTreeMap<String, i64> = fused
        .iter()
        .map(|assigned| (assigned.tool_id.clone(), assigned.relevance_q32))
        .collect();
    let mut knobs = ToolRankingKnobs::from_policy(policy);
    knobs.top_k = knobs.top_k.min(asked.top_k);
    let ranking = match rank_tools_by_relevance(
        &entries,
        &relevance,
        &asked.required_capabilities,
        asked.context_budget_tokens,
        &knobs,
    ) {
        Ok(ranking) => ranking,
        Err(refused) => return Ok(abstained(kind, &refused)),
    };
    // One scored option per selected tool, in rank order.
    let mut chosen: Vec<_> = fused
        .iter()
        .filter(|assigned| ranking.selected_tool_ids.contains(&assigned.tool_id))
        .collect();
    chosen.sort_by_key(|assigned| assigned.rank);
    let mut executed = advisory(
        chosen
            .iter()
            .map(|assigned| assigned.tool_id.clone())
            .collect(),
    )?;
    if let StatisticalOutcome::Advisory { scores, .. } = &mut executed.ladder.outcome {
        let rescored: Vec<_> = scores
            .iter()
            .zip(&chosen)
            .map(|(option, assigned)| {
                let mut option = option.clone();
                option.score.value = assigned.relevance_q32;
                option
            })
            .collect();
        *scores = BoundedVec::new(rescored)?;
    }
    let rank_text = |rank: Option<u32>| rank.map_or("none".to_string(), |rank| rank.to_string());
    let mut premises: Vec<FamilyPremise> = chosen
        .iter()
        .map(|assigned| FamilyPremise {
            subject: "tool_relevance",
            fact: format!(
                "tool_rank:tool={};lexical={};embedding={};fused_rank={};index_revision={}",
                assigned.tool_id,
                rank_text(assigned.lexical_rank),
                rank_text(assigned.embedding_rank),
                assigned.rank,
                index.generation
            ),
            class: PremiseClass::Claim,
            source: match index.definition_digest(&assigned.tool_id) {
                Some(digest) => FamilyPremiseSource::Publisher {
                    component_id: assigned.tool_id.clone(),
                    definition_digest: digest.to_string(),
                },
                None => FamilyPremiseSource::Request(PARAM_ASSIGNMENT),
            },
        })
        .collect();
    let mut digest = claim(
        format!("catalog_digest={}", index.catalog_digest),
        PARAM_ASSIGNMENT,
    );
    digest.class = PremiseClass::Definition;
    premises.push(digest);
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for ToolRelevanceFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        assignment(request)?
            .validate()
            .map_err(|error| invalid(error.to_string()))
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude(ctx, request, policy))
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

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
