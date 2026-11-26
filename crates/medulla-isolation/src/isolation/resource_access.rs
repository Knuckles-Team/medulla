//! Resource-grant decisions (MEDULLA-IDENTITY-R704.2, MEDULLA-FEDERATED-QUERY-R704.4).
//!
//! One decision function answers every question about a resource grant: the served
//! `CheckResourceAccess` method reads it as a reply, and the retrieval methods read it as a
//! gate. Access is denied by default: no unexpired grant naming the caller (or one of its
//! roles), the resource, the protocol and the operation means no access.

use super::*;
use medulla_identity_types::acl::{ResourceAccessDecision, ResourceGrantSubject, ResourceProtocol};
use medulla_types::contract::ServerErrorCode;

/// The one refusal detail of an ungranted resource. It names the resource the caller asked
/// about and no reason, so an unknown resource, a missing grant and an expired one answer
/// alike and the refusal is no existence oracle.
fn denied_detail(resource_ref: &str) -> String {
    format!("{resource_ref}: no unexpired grant")
}

impl IsolationLayer {
    /// The decision for one agent, resource, protocol and operation, under one read of the
    /// layer. The subjects are the agent and every role of its registered identity (an
    /// unregistered agent has none); no grant naming the resource is `RESOURCE_UNKNOWN`, none
    /// matching a subject, protocol and operation is `RESOURCE_GRANT_MISSING`, every match
    /// ended is `GRANT_EXPIRED`, otherwise the unexpired match with the latest expiry (ties by
    /// id) allows.
    pub fn resource_access_decision(
        &self,
        agent_id: &str,
        resource_ref: &str,
        protocol: ResourceProtocol,
        operation: &str,
        now_ms: u64,
    ) -> ResourceAccessDecision {
        let revocation_revision = self.policy_revision();
        let refusal = |code: ServerErrorCode| ResourceAccessDecision {
            allow: false,
            grant_id: None,
            scope_ceiling: Vec::new(),
            expires_at_ms: None,
            revocation_revision,
            reason_code: Some(code.as_str().to_string()),
        };
        let ledger = self.rbac().resource_grants();
        if !ledger.names_resource(resource_ref) {
            return refusal(ServerErrorCode::ResourceUnknown);
        }
        let mut subjects = vec![ResourceGrantSubject::Principal(agent_id.to_string())];
        if let Some(identity) = self.get_identity(agent_id) {
            subjects.extend(identity.roles.into_iter().map(ResourceGrantSubject::Role));
        }
        let mut matching = ledger
            .matching(&subjects, resource_ref, protocol, operation)
            .peekable();
        if matching.peek().is_none() {
            return refusal(ServerErrorCode::ResourceGrantMissing);
        }
        let live = matching
            .filter(|grant| grant.expires_at_ms > now_ms)
            .max_by(|a, b| {
                a.expires_at_ms
                    .cmp(&b.expires_at_ms)
                    .then_with(|| b.grant_id.cmp(&a.grant_id))
            });
        match live {
            Some(grant) => ResourceAccessDecision {
                allow: true,
                grant_id: Some(grant.grant_id.clone()),
                scope_ceiling: grant.scope_ceiling.clone(),
                expires_at_ms: Some(grant.expires_at_ms),
                revocation_revision,
                reason_code: None,
            },
            None => refusal(ServerErrorCode::GrantExpired),
        }
    }

    /// Refuse `RESOURCE_ACCESS_DENIED` unless `principal` holds an unexpired grant for every
    /// resource in `resources` under `protocol` and `operation`. The first resource without
    /// one is named; the reason is not (see [`denied_detail`]).
    pub fn require_resource_grants(
        &self,
        principal: &str,
        resources: &[String],
        protocol: ResourceProtocol,
        operation: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        for resource in resources {
            let decision =
                self.resource_access_decision(principal, resource, protocol, operation, now_ms);
            if !decision.allow {
                return Err(format!(
                    "{}: {}",
                    ServerErrorCode::ResourceAccessDenied.as_str(),
                    denied_detail(resource)
                ));
            }
        }
        Ok(())
    }
}
