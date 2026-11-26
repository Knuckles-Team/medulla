//! The tier a skill champion holds (MEDULLA-DECISION-ENGINE-R714.2 to R714.4).
//!
//! A tiered promotion seals the premise `tier:target=<tier>`. When the champion pointer moves on
//! that record, the tier is stored beside it under `skill-tier:<skill_ref>` by compare-and-set, so
//! the gate and the load set read one durable row per skill. The always-on set is every row whose
//! tier is `AlwaysOn`.

use std::collections::BTreeMap;

use medulla_decision_types::decision::promotion_tier::{
    SkillTier, MEMBER_OF, PERSONA_CLASS, PERSONA_INCLUDES, TIER_KEY_PREFIX,
};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};

use super::stat_support::edges_named;

/// The most skills one tenant's tier rows are read for.
const MAX_TIER_ROWS: usize = 100_000;

fn tier_key(skill_ref: &str) -> String {
    format!("{TIER_KEY_PREFIX}{skill_ref}")
}

/// The tier `skill_ref` holds, when it holds one.
pub(super) fn read_tier(
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
pub(super) fn write_tier(
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
pub(super) fn read_tiers(
    store: &AgentLibraryStore,
    tenant: &str,
) -> Result<BTreeMap<String, SkillTier>, String> {
    let mut tiers = BTreeMap::new();
    for (key, bytes) in store.decision_artifacts_with_prefix(tenant, TIER_KEY_PREFIX, MAX_TIER_ROWS)? {
        if let Some(skill_ref) = key.strip_prefix(TIER_KEY_PREFIX) {
            tiers.insert(skill_ref.to_string(), decode_artifact(&bytes, "skill tier")?);
        }
    }
    Ok(tiers)
}

/// The always-on skills, in reference order.
pub(super) fn always_on_set(tiers: &BTreeMap<String, SkillTier>) -> Vec<String> {
    tiers
        .iter()
        .filter(|(_, tier)| **tier == SkillTier::AlwaysOn)
        .map(|(skill, _)| skill.clone())
        .collect()
}

/// Whether the graph shows `principal` as a member of `team`.
pub(super) fn is_member(view: &GraphView, principal: &str, team: &str) -> bool {
    edges_named(view, MEMBER_OF)
        .iter()
        .any(|(source, target)| source == principal && target == team)
}

/// The tier a decision record promoted to: the value of its `tier:target=` premise.
pub(super) fn tier_of_premises<'a>(facts: impl IntoIterator<Item = &'a str>) -> Option<SkillTier> {
    use medulla_decision_types::decision::promotion_tier::PREMISE_TARGET;
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
