//! Who may read a stored assistant profile (MEDULLA-DECISION-ENGINE-R656.2,
//! R128.16).
//!
//! One definition of the share check and the stored-object read, used by the
//! served profile store and by the `harness_config` family so that a profile
//! is read under the same rule on both paths.

use medulla_graph_core::graph::GraphCore;
use medulla_identity_types::assistant_profile::{
    profile_node_id, profile_resource, shares_as_grants, AssistantProfile, ProfileShare,
};
use {
    medulla_identity_core_types::acl::GrantEffect, medulla_identity_core_types::acl::RbacAction,
    medulla_identity_core_types::acl::ResourceContext,
};

use crate::isolation::IsolationLayer;
use crate::rbac::RbacPolicy;

/// Refusal code of an absent or unreadable profile.
pub const PROFILE_NOT_VISIBLE: &str = "PROFILE_NOT_VISIBLE";

/// Who is asking: the verified agent id and the RBAC role names it holds.
pub struct Caller {
    /// The verified agent id.
    pub agent_id: String,
    /// The RBAC role names the agent holds.
    pub roles: Vec<String>,
}

impl Caller {
    /// The caller `agent_id` as the isolation layer knows it.
    pub fn of(isolation: &IsolationLayer, agent_id: &str) -> Self {
        let roles = isolation
            .get_identity(agent_id)
            .map(|identity| identity.roles)
            .unwrap_or_default();
        Self {
            agent_id: agent_id.to_string(),
            roles,
        }
    }
}

/// What a caller may do with one stored object: the owner may do everything;
/// any other caller is judged by the grants its shares stand for.
pub fn may(
    caller: &Caller,
    owner: &str,
    shares: &[ProfileShare],
    resource: &str,
    action: RbacAction,
) -> bool {
    if caller.agent_id == owner {
        return true;
    }
    let mut policy = RbacPolicy::new();
    for grant in shares_as_grants(shares, resource) {
        policy.add_grant(grant);
    }
    let context = ResourceContext::graph(resource);
    let effect = |action| policy.evaluate(caller.roles.as_slice(), &context, action);
    match action {
        RbacAction::Read => {
            effect(RbacAction::Read) == Some(GrantEffect::Allow)
                || effect(RbacAction::Admin) == Some(GrantEffect::Allow)
        }
        RbacAction::Write => {
            effect(RbacAction::Write) == Some(GrantEffect::Allow)
                || effect(RbacAction::Admin) == Some(GrantEffect::Allow)
        }
        RbacAction::Admin => effect(RbacAction::Admin) == Some(GrantEffect::Allow),
    }
}

/// The stored JSON document of one node whose `type` property is `label` and
/// whose tenant is `tenant_scope`; anything else reads as absent.
pub fn read_stored<T: serde::de::DeserializeOwned>(
    core: &GraphCore,
    node_id: &str,
    label: &str,
    tenant_scope: &str,
) -> Option<T> {
    let blob = core.get_node_properties(node_id)?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    if props.get("type").and_then(|value| value.as_str()) != Some(label)
        || props.get("tenant_scope").and_then(|value| value.as_str()) != Some(tenant_scope)
    {
        return None;
    }
    serde_json::from_str(props.get("json")?.as_str()?).ok()
}

/// The one non-disclosing envelope of an absent or unreadable profile.
pub fn profile_unavailable(profile_id: &str) -> String {
    format!("{PROFILE_NOT_VISIBLE}: assistant profile {profile_id} is not available to the caller")
}

/// The stored profile `profile_id` of `tenant_scope` when `agent_id` may read
/// it; an absent profile, another tenant's and one the caller may not read all
/// refuse with the same [`profile_unavailable`] envelope.
pub fn visible_profile(
    core: &GraphCore,
    isolation: &IsolationLayer,
    tenant_scope: &str,
    agent_id: &str,
    profile_id: &str,
) -> Result<AssistantProfile, String> {
    let caller = Caller::of(isolation, agent_id);
    read_stored::<AssistantProfile>(
        core,
        &profile_node_id(profile_id),
        "AssistantProfile",
        tenant_scope,
    )
    .filter(|profile| {
        may(
            &caller,
            &profile.owner_principal,
            &profile.shares,
            &profile_resource(&profile.profile_id),
            RbacAction::Read,
        )
    })
    .ok_or_else(|| profile_unavailable(profile_id))
}
