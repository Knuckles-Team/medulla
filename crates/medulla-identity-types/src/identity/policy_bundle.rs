//! Policy rule bundles: validate, bind and enforce in the engine (MEDULLA-IDENTITY-R702).
//!
//! A bundle is identity-store state. It rides the served `Method::Identity` as the op family
//! `IdentityOp::Policy`, so it takes the store's stamping, `identity:admin` check, audit
//! trail and one durable write. Binding a bundle lowers each rule to one RBAC role named
//! `idm:policy:<bundle_id>:<group>` and one grant; the RBAC projection emits them and adds
//! the role to every active member of the rule's group, so enforcement is the one access
//! boundary that already evaluates RBAC grants. The type is distinct from the row-visibility
//! export bundle of the policy export.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::projection::rbac_role_name;
use super::stamp::IdentityStamp;
use super::store::IdentityStore;
use super::text::{bounded, identifier, MAX_NAME_BYTES};
use super::views::IdentityReply;
use super::{IdentityEvent, IdentityRefusal};
use crate::acl::{Grant, GrantEffect, RbacAction, ResourceSelector, Role};

/// The one schema version of a bundle.
pub const POLICY_BUNDLE_SCHEMA_VERSION: u32 = 1;
/// Most rules in one bundle.
pub const MAX_POLICY_RULES: usize = 256;
/// Most bundles the store holds at once.
pub const MAX_POLICY_BUNDLES: usize = 1024;

/// Where a bundle applies. Under the one-tenant door `Tenant` covers every graph of the
/// deployment, so there is no separate per-tenant-graph or cluster value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicyBundleScope {
    /// Every graph of the deployment.
    Tenant,
    /// One named graph.
    Graph {
        /// The graph the bundle is bound to.
        graph: String,
    },
}

impl PolicyBundleScope {
    /// The key the store holds the applied bundle under: `"tenant"` or `"graph:<graph>"`.
    pub fn key(&self) -> String {
        match self {
            Self::Tenant => "tenant".to_string(),
            Self::Graph { graph } => format!("graph:{graph}"),
        }
    }
}

/// One rule: members of a group are allowed or denied an action over a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyRule {
    /// A group the identity store holds.
    pub principal_group: String,
    /// The action the rule allows or denies.
    pub action: RbacAction,
    /// The resource the rule covers.
    pub resource_scope: ResourceSelector,
    /// Whether the rule allows or denies.
    pub effect: GrantEffect,
}

/// A named set of rules bound to one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyRuleBundle {
    /// Always [`POLICY_BUNDLE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// An identifier of `[a-z0-9._:-]`.
    pub bundle_id: String,
    /// Where the bundle applies.
    pub scope: PolicyBundleScope,
    /// One to [`MAX_POLICY_RULES`] rules.
    pub rules: Vec<PolicyRule>,
}

/// Validate, bind or unbind a policy bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicyBundleOp {
    /// Compile the bundle and reply with the roles and grants it lowers to; writes nothing.
    Validate {
        /// The bundle to compile.
        bundle: PolicyRuleBundle,
    },
    /// Apply the bundle to its scope.
    Bind {
        /// The bundle to bind.
        bundle: PolicyRuleBundle,
    },
    /// Free a scope.
    Unbind {
        /// The scope to free.
        scope: PolicyBundleScope,
    },
}

/// What a bundle lowers to: the RBAC roles and grants the projection emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPolicy {
    /// One role per rule, `idm:policy:<bundle_id>:<group>`.
    pub roles: Vec<Role>,
    /// One grant per rule.
    pub grants: Vec<Grant>,
}

/// The RBAC role id (before the `idm:` prefix) of one rule.
fn rule_role_id(bundle_id: &str, group: &str) -> String {
    format!("policy:{bundle_id}:{group}")
}

/// The RBAC role name of one rule of a bundle.
pub(super) fn rule_role_name(bundle_id: &str, group: &str) -> String {
    rbac_role_name(&rule_role_id(bundle_id, group))
}

impl PolicyRuleBundle {
    fn check_shape(&self) -> Result<(), IdentityRefusal> {
        if self.schema_version != POLICY_BUNDLE_SCHEMA_VERSION
            || self.rules.is_empty()
            || self.rules.len() > MAX_POLICY_RULES
        {
            return Err(IdentityRefusal::InvalidRequest);
        }
        identifier(&self.bundle_id)?;
        if let PolicyBundleScope::Graph { graph } = &self.scope {
            bounded(graph, MAX_NAME_BYTES)?;
        }
        for rule in &self.rules {
            bounded(&rule.principal_group, MAX_NAME_BYTES)?;
        }
        Ok(())
    }
}

/// Lower a bundle to its roles and grants. The projection runs this on every applied bundle,
/// so it is total: a group that left the store after the bind grants nothing, because no
/// principal is a member of it.
pub fn lower(bundle: &PolicyRuleBundle) -> CompiledPolicy {
    let mut seen = BTreeSet::new();
    let mut roles = Vec::new();
    let mut grants = Vec::new();
    for rule in &bundle.rules {
        let name = rule_role_name(&bundle.bundle_id, &rule.principal_group);
        if seen.insert(name.clone()) {
            roles.push(Role::new(name.clone()));
        }
        grants.push(Grant {
            role: name,
            resource: rule.resource_scope.clone(),
            action: rule.action,
            effect: rule.effect,
        });
    }
    CompiledPolicy { roles, grants }
}

/// Compile a bundle against the store: every rule's group must exist, and under a graph scope
/// every rule's resource must be that graph or a label. A failure is
/// [`IdentityRefusal::InvalidRequest`].
pub fn compile(
    bundle: &PolicyRuleBundle,
    store: &IdentityStore,
) -> Result<CompiledPolicy, IdentityRefusal> {
    bundle.check_shape()?;
    for rule in &bundle.rules {
        if !store.groups.contains_key(&rule.principal_group) {
            return Err(IdentityRefusal::InvalidRequest);
        }
        if let PolicyBundleScope::Graph { graph } = &bundle.scope {
            match &rule.resource_scope {
                ResourceSelector::Graph(named) if named == graph => {}
                ResourceSelector::Label(_) => {}
                _ => return Err(IdentityRefusal::InvalidRequest),
            }
        }
    }
    Ok(lower(bundle))
}

impl IdentityStore {
    /// The applied bundles, by scope key.
    pub fn policy_bundles(&self) -> &std::collections::BTreeMap<String, PolicyRuleBundle> {
        &self.policy_bundles
    }

    /// Apply one policy op. The store has already authorized the op as `identity:admin`.
    pub(super) fn apply_policy(
        &mut self,
        op: &PolicyBundleOp,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        match op {
            PolicyBundleOp::Validate { bundle } => {
                let compiled = compile(bundle, self)?;
                Ok(IdentityReply::PolicyCompiled {
                    roles: compiled.roles,
                    grants: compiled.grants,
                })
            }
            PolicyBundleOp::Bind { bundle } => self.bind_policy(bundle, stamp, now_ms),
            PolicyBundleOp::Unbind { scope } => self.unbind_policy(scope, stamp, now_ms),
        }
    }

    fn bind_policy(
        &mut self,
        bundle: &PolicyRuleBundle,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        compile(bundle, self)?;
        self.refuse_self_authorization(bundle, &stamp.actor.principal_id)?;
        let key = bundle.scope.key();
        if let Some(bound) = self.policy_bundles.get(&key) {
            if bound.bundle_id != bundle.bundle_id {
                return Err(IdentityRefusal::PolicyScopeBound);
            }
            if bound == bundle {
                return Ok(IdentityReply::Done { changed: false });
            }
        }
        // Two scopes sharing a bundle id would share role names, and so merge their grants.
        if self
            .policy_bundles
            .iter()
            .any(|(other, bound)| *other != key && bound.bundle_id == bundle.bundle_id)
        {
            return Err(IdentityRefusal::Collision);
        }
        if !self.policy_bundles.contains_key(&key) && self.policy_bundles.len() >= MAX_POLICY_BUNDLES
        {
            return Err(IdentityRefusal::Full);
        }
        self.policy_bundles.insert(key.clone(), bundle.clone());
        self.audit_event(stamp, now_ms, IdentityEvent::PolicyBound, Some(&key));
        Ok(IdentityReply::Done { changed: true })
    }

    fn unbind_policy(
        &mut self,
        scope: &PolicyBundleScope,
        stamp: &IdentityStamp,
        now_ms: u64,
    ) -> Result<IdentityReply, IdentityRefusal> {
        let key = scope.key();
        if self.policy_bundles.remove(&key).is_none() {
            return Err(IdentityRefusal::NotFound);
        }
        self.audit_event(stamp, now_ms, IdentityEvent::PolicyUnbound, Some(&key));
        Ok(IdentityReply::Done { changed: true })
    }

    /// No caller raises its own authority by the bundle it binds: a rule that allows `Admin`
    /// to a group containing the binding actor is refused.
    fn refuse_self_authorization(
        &self,
        bundle: &PolicyRuleBundle,
        actor: &str,
    ) -> Result<(), IdentityRefusal> {
        let raises = bundle.rules.iter().any(|rule| {
            rule.action == RbacAction::Admin
                && rule.effect == GrantEffect::Allow
                && self
                    .groups
                    .get(&rule.principal_group)
                    .is_some_and(|group| group.members.contains_key(actor))
        });
        if raises {
            Err(IdentityRefusal::PolicySelfAuthorization)
        } else {
            Ok(())
        }
    }

    /// The `idm:policy:` role names the applied bundles give a principal who belongs to
    /// `groups`.
    pub(super) fn policy_roles_for(&self, groups: &BTreeSet<String>) -> BTreeSet<String> {
        self.policy_bundles
            .values()
            .flat_map(|bundle| {
                bundle
                    .rules
                    .iter()
                    .filter(|rule| groups.contains(&rule.principal_group))
                    .map(|rule| rule_role_name(&bundle.bundle_id, &rule.principal_group))
            })
            .collect()
    }
}
