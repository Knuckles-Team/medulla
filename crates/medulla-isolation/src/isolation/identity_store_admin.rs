//! The identity store over the RBAC image.
//!
//! An identity op is applied to a CLONE of the store; a refusal therefore
//! changes nothing. An accepted op that changed the store is projected into
//! the RBAC roles, grants and identities it owns and written through with the
//! whole authorization image in ONE durable write, rolled back if the write
//! fails -- so a role removed in the store is removed from RBAC atomically.
//!
//! The two older writers are audited into the same trail and fenced off the
//! store's namespace: `RegisterIdentity` cannot overwrite a store-managed
//! principal, and `RbacAdmin` cannot touch an `idm:` role or grant.
#![cfg(feature = "security")]

use medulla_identity_types::{
    identity::ApplyContext, identity::AuditRecord, identity::ConfigOp, identity::DenialSample,
    identity::IdentityEvent, identity::IdentityOp, identity::IdentityRefusal,
    identity::IdentityReply, identity::IdentityStamp, identity::IdentityStore,
    identity::ScopeClassifier, identity::RBAC_ROLE_PREFIX,
};
use medulla_types::acl::{Grant, RbacAdminOp, Role};

use super::{AgentIdentity, AgentRole, IsolationLayer};

/// The agent a System-identity repair names, if `op` is one.
fn system_repair_target(op: &IdentityOp) -> Option<&str> {
    if let IdentityOp::Config(ConfigOp::RepairSystemIdentity { request }) = op {
        Some(&request.id)
    } else {
        None
    }
}

/// A refused identity op, or a persistence failure after an accepted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityStoreError {
    Refused(IdentityRefusal),
    /// The engine's System identity has not bootstrapped yet: the store may
    /// hold only its seed until it has (see `bootstrap_order`).
    SystemBootstrapPending,
    Persist(String),
}

impl std::fmt::Display for IdentityStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::SystemBootstrapPending => f.write_str(SYSTEM_BOOTSTRAP_PENDING),
            Self::Persist(error) => f.write_str(error),
        }
    }
}

/// Refusal for a real principal, credential or grant (first-admin setup, a
/// claim, SCIM/LDAP provisioning, ...) before the engine's System identity
/// bootstrapped: were it accepted, the System bootstrap could never run.
pub const SYSTEM_BOOTSTRAP_PENDING: &str =
    "IDENTITY_SYSTEM_BOOTSTRAP_PENDING: register the engine's System identity (the signer-backed bootstrap) before creating principals, credentials or grants";

/// Refusal codes for the older writers entering the store's namespace.
pub const STORE_MANAGED: &str =
    "IDENTITY_STORE_MANAGED: this principal is owned by the identity store; change it through Method::Identity";
pub const STORE_NAMESPACE: &str =
    "IDENTITY_STORE_NAMESPACE: idm: roles and grants are owned by the identity store";

/// Who performed an RBAC write, for the identity audit trail.
#[derive(Debug, Clone)]
pub struct AuditActor<'a> {
    pub principal: &'a str,
    pub now_ms: u64,
    /// The tenant the write was made under, for the audit entry (`None`
    /// when the caller has no verified tenant in scope yet).
    pub tenant: Option<String>,
}

/// An audit annotation (MEDULLA-IDENTITY-R002) that must commit in the
/// policy store save the write triggers: the tenant and the before/after
/// authorization digests for the entries recorded after `before_seq`.
/// `expected_last_seq` is the trail's newest seq when the annotation was
/// staged; a save reached after further entries were recorded drops the
/// annotation instead of naming the wrong entries.
#[derive(Debug, Clone)]
pub(crate) struct PendingAuditAnnotate {
    pub(crate) before_seq: Option<u64>,
    pub(crate) expected_last_seq: Option<u64>,
    pub(crate) tenant: Option<String>,
    pub(crate) before_digest: String,
}

impl IsolationLayer {
    /// Apply one identity op against the store and its RBAC projection.
    pub fn try_apply_identity(
        &mut self,
        op: &IdentityOp,
        stamp: &IdentityStamp,
        now_ms: u64,
        classifier: &dyn ScopeClassifier,
    ) -> Result<IdentityReply, IdentityStoreError> {
        if matches!(
            op,
            IdentityOp::Session(
                medulla_identity_types::identity::SessionOp::InspectReference { .. }
                    | medulla_identity_types::identity::SessionOp::RenewReference { .. }
            )
        ) {
            return self.apply_session_authority(op, stamp, now_ms, classifier);
        }
        self.apply_identity_image(op, stamp, now_ms, classifier)
    }

    pub(super) fn apply_identity_image(
        &mut self,
        op: &IdentityOp,
        stamp: &IdentityStamp,
        now_ms: u64,
        classifier: &dyn ScopeClassifier,
    ) -> Result<IdentityReply, IdentityStoreError> {
        let mut next = self.rbac.identity_store().clone();
        let ctx = ApplyContext { now_ms, classifier };
        let reply = next
            .apply(op, stamp, &ctx)
            .map_err(IdentityStoreError::Refused)?;
        self.bootstrap_order(op, &next)?;
        self.refuse_takeover(&next)?;
        if &next == self.rbac.identity_store() {
            return Ok(reply);
        }
        // The one-time System bootstrap is NOT consumed here: the identity
        // store owns only its own principals, and the engine's System
        // identity is still registered through the dedicated bootstrap path
        // (or, after it, repaired by an administrator below).
        let previous = (self.rbac.clone(), self.agents.clone());
        let before_seq = self.rbac.identity_store().audit_trail().last_seq();
        let before_digest = self.image_digest();
        *self.rbac.identity_store_mut() = next;
        self.project_identity_store();
        if let Some(agent_id) = system_repair_target(op) {
            self.merge_system_identity(agent_id);
        }
        // The tenant and the change of the authorization digest the
        // recompute produced commit in this same save (R002): the write
        // applies the staged annotation before it stores the image.
        self.pending_audit_annotate = Some(PendingAuditAnnotate {
            before_seq,
            expected_last_seq: self.rbac.identity_store().audit_trail().last_seq(),
            tenant: stamp.session_tenant.clone(),
            before_digest,
        });
        if let Err(error) = self.persist_state() {
            (self.rbac, self.agents) = previous;
            return Err(IdentityStoreError::Persist(error));
        }
        Ok(reply)
    }

    /// Ordering: until the engine's System identity bootstrapped, the store
    /// may hold only its SEED -- a first administrator, a provisioned user or
    /// any other real principal, credential or grant is refused, and so is a
    /// System-identity repair (the bootstrap itself is the path then).
    fn bootstrap_order(
        &self,
        op: &IdentityOp,
        next: &IdentityStore,
    ) -> Result<(), IdentityStoreError> {
        let pending =
            self.identity_bootstrap == crate::rbac_persist::IdentityBootstrapState::Pending;
        let leaves_seed = system_repair_target(op).is_some() || !next.holds_only_seed();
        if pending && leaves_seed {
            return Err(IdentityStoreError::SystemBootstrapPending);
        }
        Ok(())
    }

    /// Takeover fence: the store may not start managing a principal that
    /// already holds an identity registered outside it (`RegisterIdentity`,
    /// the System bootstrap, a signer). The projection REPLACES a managed
    /// principal's identity, so accepting would strip its System role and
    /// non-`idm:` roles, and `RegisterIdentity` could not restore them.
    fn refuse_takeover(&self, next: &IdentityStore) -> Result<(), IdentityStoreError> {
        let current = self.rbac.identity_store();
        let taken = self
            .agents
            .keys()
            .any(|principal| next.manages(principal) && !current.manages(principal));
        if taken {
            return Err(IdentityStoreError::Refused(IdentityRefusal::Collision));
        }
        Ok(())
    }

    /// Make `agent_id` the System identity, keeping its teams and roles (a
    /// repair never replaces unrelated roles; a new agent starts with none).
    fn merge_system_identity(&mut self, agent_id: &str) {
        let identity = self
            .agents
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentIdentity {
                agent_id: agent_id.to_string(),
                role: AgentRole::System,
                teams: Vec::new(),
                roles: Vec::new(),
            });
        identity.role = AgentRole::System;
    }

    /// Replace the RBAC state the store owns with its current projection:
    /// every `idm:` role and grant, and the FULL identity of every principal
    /// the store manages (an inactive principal's identity is removed).
    pub(super) fn project_identity_store(&mut self) {
        let projection = self.rbac.identity_store().rbac_projection();
        self.rbac
            .replace_projected(projection.roles, projection.grants);
        for principal in &projection.managed {
            self.agents.remove(principal);
        }
        for (principal, identity) in projection.identities {
            self.agents.insert(principal, identity);
        }
    }

    /// `RegisterIdentity` with its audit entry, written in the same image.
    /// A store-managed principal is refused: the store owns its identity.
    pub fn try_register_agent_audited(
        &mut self,
        identity: AgentIdentity,
        actor: AuditActor<'_>,
    ) -> Result<(), String> {
        if self.rbac.identity_store().manages(&identity.agent_id) {
            return Err(STORE_MANAGED.to_string());
        }
        if identity
            .roles
            .iter()
            .any(|role| role.starts_with(RBAC_ROLE_PREFIX))
        {
            return Err(STORE_NAMESPACE.to_string());
        }
        let target = identity.agent_id.clone();
        self.audited(
            actor,
            IdentityEvent::RbacIdentityRegistered,
            &target,
            |layer| layer.try_register_agent_from_request(identity),
        )
    }

    /// `RbacAdmin` role/grant writes with their audit entry. The store's
    /// `idm:` namespace is refused.
    pub fn try_rbac_admin_audited(
        &mut self,
        op: RbacAdminOp,
        actor: AuditActor<'_>,
    ) -> Result<bool, String> {
        let (target, touches_store) = match &op {
            RbacAdminOp::AddRole(role) => (role.name.clone(), owned_role(role)),
            RbacAdminOp::RemoveRole(name) => (name.clone(), name.starts_with(RBAC_ROLE_PREFIX)),
            RbacAdminOp::AddGrant(grant) | RbacAdminOp::RemoveGrant(grant) => {
                (grant.role.clone(), owned_grant(grant))
            }
            RbacAdminOp::List => return Ok(false),
        };
        if touches_store {
            return Err(STORE_NAMESPACE.to_string());
        }
        let mut changed = true;
        self.audited(
            actor,
            IdentityEvent::RbacPolicyChanged,
            &target,
            |layer| match op {
                RbacAdminOp::AddRole(role) => layer.try_add_role(role),
                RbacAdminOp::RemoveRole(name) => layer.try_remove_role(&name),
                RbacAdminOp::AddGrant(grant) => layer.try_add_grant(grant),
                RbacAdminOp::RemoveGrant(grant) => {
                    changed = layer.try_remove_grant(&grant)?;
                    // An absent grant skips the underlying save, but this
                    // request still appended an audit record. Commit it too.
                    if !changed {
                        layer.persist_state()?;
                    }
                    Ok(())
                }
                RbacAdminOp::List => Ok(()),
            },
        )?;
        Ok(changed)
    }

    /// The canonical digest of the current authorization image (policy and
    /// identities together), for the audit trail; empty when it cannot be
    /// computed.
    fn image_digest(&self) -> String {
        {
            let identities: std::collections::BTreeMap<String, AgentIdentity> = self
                .agents
                .iter()
                .map(|(id, identity)| (id.clone(), identity.clone()))
                .collect();
            crate::rbac_persist::authorization_digest(self.rbac(), &identities).unwrap_or_default()
        }
    }

    /// Run `write` (which persists) with an audit entry appended to the
    /// identity trail BEFORE it, so the entry rides the write's own save and
    /// is rolled back with it. The entry's tenant and the change of the
    /// authorization digest are staged as a pending annotation that the
    /// write's own save commits before it writes the image; the fields sit
    /// outside the chain link, so annotating never disturbs a successor.
    fn audited(
        &mut self,
        actor: AuditActor<'_>,
        event: IdentityEvent,
        target: &str,
        write: impl FnOnce(&mut Self) -> Result<(), String>,
    ) -> Result<(), String> {
        let previous = self.rbac.identity_store().clone();
        let before_seq = self.rbac.identity_store().audit_trail().last_seq();
        let before_digest = self.image_digest();
        self.rbac.identity_store_mut().record(AuditRecord {
            at_ms: actor.now_ms,
            actor: actor.principal.to_string(),
            event,
            target: Some(target.to_string()),
            ip_prefix: None,
            detail: String::new(),
            tenant: actor.tenant.clone(),
            authorization_digest: None,
        });
        self.pending_audit_annotate = Some(PendingAuditAnnotate {
            before_seq,
            expected_last_seq: self.rbac.identity_store().audit_trail().last_seq(),
            tenant: actor.tenant.clone(),
            before_digest,
        });
        let outcome = write(self);
        if outcome.is_err() {
            *self.rbac.identity_store_mut() = previous;
        }
        outcome
    }

    /// Offer one denial to this layer's sampler. Never blocks on durable
    /// state and never fails the denial it describes.
    pub fn offer_denial(&self, principal: &str, action: &str, reason: &str) {
        let sample = DenialSample {
            at_ms: super::access_clock_ms(),
            principal: principal.to_string(),
            action: action.to_string(),
            reason: reason.to_string(),
        };
        if let Ok(mut sampler) = self.denials.lock() {
            sampler.offer(sample);
        }
    }

    /// Take the kept denial samples and the count dropped since the last drain.
    pub fn drain_denials(&self) -> (Vec<DenialSample>, u64) {
        self.denials
            .lock()
            .map(|mut sampler| sampler.drain())
            .unwrap_or_default()
    }

    /// Write a drained denial sample into the identity trail in one save.
    pub fn try_record_denials(
        &mut self,
        samples: Vec<DenialSample>,
        dropped: u64,
    ) -> Result<(), String> {
        if samples.is_empty() && dropped == 0 {
            return Ok(());
        }
        let previous = self.rbac.identity_store().clone();
        let last_at = samples.last().map_or(0, |sample| sample.at_ms);
        let store = self.rbac.identity_store_mut();
        for sample in samples {
            store.record(AuditRecord {
                at_ms: sample.at_ms,
                actor: sample.principal,
                event: IdentityEvent::AccessDenied,
                target: Some(sample.action),
                ip_prefix: None,
                detail: sample.reason,
                tenant: None,
                authorization_digest: None,
            });
        }
        if dropped > 0 {
            store.record(AuditRecord {
                at_ms: last_at,
                actor: "engine".to_string(),
                event: IdentityEvent::AccessDenied,
                target: None,
                ip_prefix: None,
                detail: format!("dropped={dropped}"),
                tenant: None,
                authorization_digest: None,
            });
        }
        if let Err(error) = self.persist_state() {
            *self.rbac.identity_store_mut() = previous;
            return Err(error);
        }
        Ok(())
    }
}

fn owned_role(role: &Role) -> bool {
    role.name.starts_with(RBAC_ROLE_PREFIX)
        || role
            .parents
            .iter()
            .any(|parent| parent.starts_with(RBAC_ROLE_PREFIX))
}

fn owned_grant(grant: &Grant) -> bool {
    grant.role.starts_with(RBAC_ROLE_PREFIX)
}

#[path = "identity_store_admin/session_authority.rs"]
mod session_authority;
