//! The tier a skill champion holds (MEDULLA-DECISION-ENGINE-R714.2 to R714.4).
//!
//! A tiered promotion seals the premise `tier:target=<tier>`. When the champion pointer moves on
//! that record, the tier is stored beside it under `skill-tier:<skill_ref>` by compare-and-set, so
//! the gate and the load set read one durable row per skill. The always-on set is every row whose
//! tier is `AlwaysOn`.

use std::collections::BTreeMap;

use medulla_decision_types::decision::statistical::{DecideRequest, TypedParam, TypedValue};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::promotion_tier::SkillTier,
    medulla_decision_statistical_types::decision::promotion_tier::MEMBER_OF,
    medulla_decision_statistical_types::decision::promotion_tier::PARAM_TIER_REVIEWER,
    medulla_decision_statistical_types::decision::promotion_tier::PERSONA_CLASS,
    medulla_decision_statistical_types::decision::promotion_tier::PERSONA_INCLUDES,
    medulla_decision_statistical_types::decision::promotion_tier::SCOPE_REVIEW,
    medulla_decision_statistical_types::decision::promotion_tier::TIER_KEY_PREFIX,
    medulla_decision_statistical_types::decision::promotion_tier::TIER_REVIEW_SCOPE_REQUIRED,
};

use super::stat_support::edges_named;

/// The most skills one tenant's tier rows are read for.
const MAX_TIER_ROWS: usize = 100_000;

fn tier_key(skill_ref: &str) -> String {
    format!("{TIER_KEY_PREFIX}{skill_ref}")
}

/// The tier `skill_ref` holds, when it holds one.
pub fn read_tier(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
) -> Result<Option<SkillTier>, String> {
    store
        .decision_artifact(tenant, &tier_key(skill_ref))?
        .map(|bytes| decode_artifact::<SkillTier>(&bytes, "skill tier"))
        .transpose()
}

/// Store the tier of `skill_ref`; a row that already holds it is left alone.
pub fn write_tier(
    store: &AgentLibraryStore,
    tenant: &str,
    skill_ref: &str,
    tier: SkillTier,
) -> Result<(), String> {
    let key = tier_key(skill_ref);
    let current = store.decision_artifact(tenant, &key)?;
    let bytes = encode_artifact(&tier)?;
    if current.as_deref() != Some(bytes.as_slice()) {
        store.swap_decision_artifact(tenant, &key, current, bytes)?;
    }
    Ok(())
}

/// Every skill of `tenant` that holds a tier, with the tier.
pub fn read_tiers(
    store: &AgentLibraryStore,
    tenant: &str,
) -> Result<BTreeMap<String, SkillTier>, String> {
    let mut tiers = BTreeMap::new();
    for (key, bytes) in
        store.decision_artifacts_with_prefix(tenant, TIER_KEY_PREFIX, MAX_TIER_ROWS)?
    {
        if let Some(skill_ref) = key.strip_prefix(TIER_KEY_PREFIX) {
            tiers.insert(
                skill_ref.to_string(),
                decode_artifact(&bytes, "skill tier")?,
            );
        }
    }
    Ok(tiers)
}

/// The always-on skills, in reference order.
pub fn always_on_set(tiers: &BTreeMap<String, SkillTier>) -> Vec<String> {
    tiers
        .iter()
        .filter(|(_, tier)| **tier == SkillTier::AlwaysOn)
        .map(|(skill, _)| skill.clone())
        .collect()
}

/// Whether the graph shows `principal` as a member of `team`.
pub fn is_member(view: &GraphView, principal: &str, team: &str) -> bool {
    edges_named(view, MEMBER_OF)
        .iter()
        .any(|(source, target)| source == principal && target == team)
}

/// The review chokepoint of a tiered skill promotion (MEDULLA-IDENTITY-R739). A request that
/// carries a `tier_review` needs the scope `skills:review`; the reviewer of a person is the
/// verified caller, bound here as the parameter `tier_reviewer` whatever the request named, so a
/// person cannot review as another principal. A service or agent keeps the approver its review
/// names. Whether the reviewer reviews for the platform or for a team is then read from the
/// graph by the promotion family, and the author is refused there.
pub fn bind_reviewer(
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let mut params = request.params.as_slice().to_vec();
    params.retain(|param| param.name != PARAM_TIER_REVIEWER);
    if params.iter().any(|param| param.name == "tier_review") {
        if !verified.allows_action(SCOPE_REVIEW) {
            return Err(format!(
                "{}: a tiered promotion needs the scope {SCOPE_REVIEW}",
                TIER_REVIEW_SCOPE_REQUIRED.as_str()
            ));
        }
        if !verified.is_loop_principal() {
            params.push(TypedParam {
                name: PARAM_TIER_REVIEWER.to_owned(),
                value: TypedValue::Text(verified.agent_id().to_owned()),
            });
        }
    }
    request.params = BoundedVec::new(params).map_err(super::stat_support::invalid_parameter)?;
    Ok(request)
}

/// Every team the graph shows `principal` as a member of, sorted.
pub fn teams_of(view: &GraphView, principal: &str) -> std::collections::BTreeSet<String> {
    edges_named(view, MEMBER_OF)
        .into_iter()
        .filter(|(source, _)| source == principal)
        .map(|(_, team)| team)
        .collect()
}

/// Whether `principal` authored the skill revision node `skill_ref`: the node names it as its
/// owner or author. A revision the view does not hold has no known author, so nobody is it.
pub fn is_author(view: &GraphView, skill_ref: &str, principal: &str) -> bool {
    view.node_row_object(skill_ref).is_some_and(|row| {
        ["_owner", "_owner_id", "authored_by"]
            .iter()
            .filter_map(|key| row.get(*key).and_then(|value| value.as_str()))
            .any(|found| found == principal)
    })
}

/// The tier a decision record promoted to: the value of its `tier:target=` premise.
pub(super) fn tier_of_premises<'a>(facts: impl IntoIterator<Item = &'a str>) -> Option<SkillTier> {
    use medulla_decision_statistical_types::decision::promotion_tier::PREMISE_TARGET;
    facts
        .into_iter()
        .find_map(|fact| fact.strip_prefix(PREMISE_TARGET))
        .and_then(SkillTier::parse)
}

/// The persona nodes of the view with the skill revisions each includes, both sorted. A persona
/// that includes nothing is present with an empty list.
pub(super) fn personas_of(view: &GraphView) -> BTreeMap<String, Vec<String>> {
    let mut personas: BTreeMap<String, Vec<String>> = view
        .node_properties
        .keys()
        .filter(|id| {
            view.node_row_object(id).is_some_and(|row| {
                ["type", "node_type"]
                    .iter()
                    .any(|key| row.get(*key).and_then(|v| v.as_str()) == Some(PERSONA_CLASS))
            })
        })
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    for (source, target) in edges_named(view, PERSONA_INCLUDES) {
        if let Some(included) = personas.get_mut(&source) {
            included.push(target);
        }
    }
    personas
}
