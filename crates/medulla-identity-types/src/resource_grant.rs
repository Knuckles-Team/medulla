//! The resource grant ledger (MEDULLA-IDENTITY-R704.1).
//!
//! A resource grant lets one principal or role call one operation of one protocol resource
//! until it expires. The ledger is part of the RBAC policy image, so grants persist in the one
//! policy write with no table of their own. Matching is exact on resource, protocol and
//! operation; the ledger holds no clock, so expiry is the caller's check.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::acl::{ResourceGrant, ResourceGrantSubject, ResourceProtocol};

/// Every resource grant, by grant id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGrantLedger {
    grants: BTreeMap<String, ResourceGrant>,
}

impl ResourceGrantLedger {
    /// Whether the ledger holds no grant (an empty ledger is left out of the policy image).
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// The number of grants held.
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// Record a grant. An exact repeat is a no-op; a different grant under the same id cannot
    /// exist, because the id is derived from the grant's own fields.
    pub fn insert(&mut self, grant: ResourceGrant) {
        self.grants.insert(grant.grant_id.clone(), grant);
    }

    /// Remove a grant by id, returning it when it was held.
    pub fn remove(&mut self, grant_id: &str) -> Option<ResourceGrant> {
        self.grants.remove(grant_id)
    }

    /// The grants for any of `subjects` on exactly this resource, protocol and operation.
    pub fn matching<'a>(
        &'a self,
        subjects: &'a [ResourceGrantSubject],
        resource_ref: &'a str,
        protocol: ResourceProtocol,
        operation: &'a str,
    ) -> impl Iterator<Item = &'a ResourceGrant> + 'a {
        self.grants.values().filter(move |grant| {
            subjects.contains(&grant.subject)
                && grant.resource_ref == resource_ref
                && grant.protocol == protocol
                && grant.operation == operation
        })
    }

    /// Whether any grant names `resource_ref`, whatever its protocol or operation.
    pub fn names_resource(&self, resource_ref: &str) -> bool {
        self.grants
            .values()
            .any(|grant| grant.resource_ref == resource_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityRefusal;

    fn grant(
        subject: ResourceGrantSubject,
        resource: &str,
        operation: &str,
    ) -> Result<ResourceGrant, IdentityRefusal> {
        ResourceGrant::new(
            subject,
            resource.to_string(),
            ResourceProtocol::Mcp,
            operation.to_string(),
            vec!["kg:read".to_string(), "kg:write".to_string()],
            1_000,
            2_000,
        )
    }

    fn protocol_grant(resource: &str, protocol: ResourceProtocol) -> ResourceGrant {
        ResourceGrant::new(
            alice(),
            resource.to_string(),
            protocol,
            "connect".to_string(),
            Vec::new(),
            1,
            2,
        )
        .expect("a valid grant")
    }

    fn alice() -> ResourceGrantSubject {
        ResourceGrantSubject::Principal("usr:alice".to_string())
    }

    // spec: MEDULLA-IDENTITY-R704.1
    #[test]
    fn grant_validates_and_ledger_matches_exactly() {
        let valid = grant(alice(), "github-mcp", "create_issue").unwrap();
        assert!(valid.grant_id.starts_with("rgrant:"));
        assert_eq!(valid.grant_id.len(), "rgrant:".len() + 64);
        // The id is stable, and every field of its derivation changes it.
        assert_eq!(
            valid.grant_id,
            grant(alice(), "github-mcp", "create_issue").unwrap().grant_id
        );
        for other in [
            grant(
                ResourceGrantSubject::Role("usr:alice".to_string()),
                "github-mcp",
                "create_issue",
            )
            .unwrap(),
            grant(alice(), "gitlab-mcp", "create_issue").unwrap(),
            grant(alice(), "github-mcp", "close_issue").unwrap(),
            ResourceGrant::new(
                alice(),
                "github-mcp".to_string(),
                ResourceProtocol::A2a,
                "create_issue".to_string(),
                Vec::new(),
                1_000,
                2_000,
            )
            .unwrap(),
            ResourceGrant::new(
                alice(),
                "github-mcp".to_string(),
                ResourceProtocol::Mcp,
                "create_issue".to_string(),
                Vec::new(),
                1_001,
                2_000,
            )
            .unwrap(),
        ] {
            assert_ne!(other.grant_id, valid.grant_id);
        }
        valid.validate().unwrap();

        // Each invalid field is refused.
        let refused = |result: Result<ResourceGrant, IdentityRefusal>| {
            assert_eq!(result, Err(IdentityRefusal::InvalidRequest));
        };
        refused(grant(
            ResourceGrantSubject::Principal("bad principal".to_string()),
            "github-mcp",
            "create_issue",
        ));
        refused(grant(alice(), "", "create_issue"));
        refused(grant(alice(), "github mcp", "create_issue"));
        refused(grant(alice(), &"x".repeat(257), "create_issue"));
        refused(grant(alice(), "github-mcp", ""));
        refused(grant(alice(), "github-mcp", "create\u{0}issue"));
        let ceiling = |scopes: Vec<String>, expires: u64| {
            ResourceGrant::new(
                alice(),
                "github-mcp".to_string(),
                ResourceProtocol::Mcp,
                "create_issue".to_string(),
                scopes,
                1_000,
                expires,
            )
        };
        refused(ceiling(vec!["b".into(), "a".into()], 2_000));
        refused(ceiling(vec!["a".into(), "a".into()], 2_000));
        refused(ceiling((0..65).map(|n| format!("scope:{n:03}")).collect(), 2_000));
        assert!(ceiling((0..64).map(|n| format!("scope:{n:03}")).collect(), 2_000).is_ok());
        refused(ceiling(Vec::new(), 1_000));
        refused(ceiling(Vec::new(), 999));

        // A grant whose id does not derive from its fields does not validate.
        let mut forged = valid.clone();
        forged.operation = "delete_repo".to_string();
        assert_eq!(forged.validate(), Err(IdentityRefusal::InvalidRequest));

        // The wire form rejects an unknown field.
        let mut wire = serde_json::to_value(&valid).unwrap();
        wire["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ResourceGrant>(wire).is_err());

        // The ledger matches only the exact resource, protocol and operation.
        let mut ledger = ResourceGrantLedger::default();
        assert!(ledger.is_empty());
        ledger.insert(valid.clone());
        ledger.insert(valid.clone());
        assert_eq!(ledger.len(), 1);
        let subjects = [alice(), ResourceGrantSubject::Role("reader".to_string())];
        let hits = |resource: &str, protocol, operation: &str| {
            ledger
                .matching(&subjects, resource, protocol, operation)
                .count()
        };
        assert_eq!(hits("github-mcp", ResourceProtocol::Mcp, "create_issue"), 1);
        assert_eq!(hits("github-mcp", ResourceProtocol::A2a, "create_issue"), 0);
        assert_eq!(hits("github-mcp", ResourceProtocol::Mcp, "close_issue"), 0);
        assert_eq!(hits("gitlab-mcp", ResourceProtocol::Mcp, "create_issue"), 0);
        assert_eq!(
            ledger
                .matching(
                    &[ResourceGrantSubject::Principal("usr:bob".to_string())],
                    "github-mcp",
                    ResourceProtocol::Mcp,
                    "create_issue"
                )
                .count(),
            0
        );
        assert!(ledger.names_resource("github-mcp"));
        assert!(!ledger.names_resource("gitlab-mcp"));

        // Remove returns the grant once; the ledger is then empty again.
        assert_eq!(ledger.remove(&valid.grant_id), Some(valid.clone()));
        assert_eq!(ledger.remove(&valid.grant_id), None);
        assert!(ledger.is_empty());
    }

    // spec: MEDULLA-IDENTITY-R704.3
    #[test]
    fn ssh_protocol_grants_match_only_their_own_protocol() {
        assert_eq!(
            serde_json::to_value(ResourceProtocol::Ssh).unwrap(),
            serde_json::json!("ssh")
        );
        let granted = protocol_grant("ssh-resource", ResourceProtocol::Ssh);
        let other = protocol_grant("ssh-resource", ResourceProtocol::Mcp);
        // The protocol is part of the grant id, so no two protocols share one.
        assert_ne!(granted.grant_id, other.grant_id);
        let mut ledger = ResourceGrantLedger::default();
        ledger.insert(granted);
        let subjects = [alice()];
        let hits = |protocol| {
            ledger
                .matching(&subjects, "ssh-resource", protocol, "connect")
                .count()
        };
        assert_eq!(hits(ResourceProtocol::Ssh), 1);
        assert_eq!(hits(ResourceProtocol::Mcp), 0);
        assert_eq!(hits(ResourceProtocol::A2a), 0);
        assert_eq!(hits(ResourceProtocol::Knowledge), 0);
    }

    // spec: MEDULLA-IDENTITY-R704.4
    #[test]
    fn database_protocol_grants_match_only_their_own_protocol() {
        assert_eq!(
            serde_json::to_value(ResourceProtocol::Database).unwrap(),
            serde_json::json!("database")
        );
        let granted = protocol_grant("database-resource", ResourceProtocol::Database);
        let other = protocol_grant("database-resource", ResourceProtocol::Mcp);
        // The protocol is part of the grant id, so no two protocols share one.
        assert_ne!(granted.grant_id, other.grant_id);
        let mut ledger = ResourceGrantLedger::default();
        ledger.insert(granted);
        let subjects = [alice()];
        let hits = |protocol| {
            ledger
                .matching(&subjects, "database-resource", protocol, "connect")
                .count()
        };
        assert_eq!(hits(ResourceProtocol::Database), 1);
        assert_eq!(hits(ResourceProtocol::Mcp), 0);
        assert_eq!(hits(ResourceProtocol::A2a), 0);
        assert_eq!(hits(ResourceProtocol::Knowledge), 0);
    }

    // spec: MEDULLA-IDENTITY-R704.5
    #[test]
    fn web_protocol_grants_match_only_their_own_protocol() {
        assert_eq!(
            serde_json::to_value(ResourceProtocol::Web).unwrap(),
            serde_json::json!("web")
        );
        let granted = protocol_grant("web-resource", ResourceProtocol::Web);
        let other = protocol_grant("web-resource", ResourceProtocol::Mcp);
        // The protocol is part of the grant id, so no two protocols share one.
        assert_ne!(granted.grant_id, other.grant_id);
        let mut ledger = ResourceGrantLedger::default();
        ledger.insert(granted);
        let subjects = [alice()];
        let hits = |protocol| {
            ledger
                .matching(&subjects, "web-resource", protocol, "connect")
                .count()
        };
        assert_eq!(hits(ResourceProtocol::Web), 1);
        assert_eq!(hits(ResourceProtocol::Mcp), 0);
        assert_eq!(hits(ResourceProtocol::A2a), 0);
        assert_eq!(hits(ResourceProtocol::Knowledge), 0);
    }

    // spec: MEDULLA-IDENTITY-R704.6
    #[test]
    fn rdp_protocol_grants_match_only_their_own_protocol() {
        assert_eq!(
            serde_json::to_value(ResourceProtocol::Rdp).unwrap(),
            serde_json::json!("rdp")
        );
        let granted = protocol_grant("rdp-resource", ResourceProtocol::Rdp);
        let other = protocol_grant("rdp-resource", ResourceProtocol::Mcp);
        // The protocol is part of the grant id, so no two protocols share one.
        assert_ne!(granted.grant_id, other.grant_id);
        let mut ledger = ResourceGrantLedger::default();
        ledger.insert(granted);
        let subjects = [alice()];
        let hits = |protocol| {
            ledger
                .matching(&subjects, "rdp-resource", protocol, "connect")
                .count()
        };
        assert_eq!(hits(ResourceProtocol::Rdp), 1);
        assert_eq!(hits(ResourceProtocol::Mcp), 0);
        assert_eq!(hits(ResourceProtocol::A2a), 0);
        assert_eq!(hits(ResourceProtocol::Knowledge), 0);
    }
}
