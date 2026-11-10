//! The identity audit trail: a bounded, hash-chained record of every
//! identity event (§5.2), written in the same durable image as the change it
//! records. No entry carries a secret or a sealed value.
//!
//! MEDULLA-IDENTITY-R002: an entry may additionally name the tenant the
//! change was made under and the change of the canonical authorization
//! digest across the write (before and after the recomputed projection).
//! Those two fields are recorded beside the chained core (`seq`, `at_ms`,
//! `actor`, `event`, `target`, `ip_prefix`, `detail`) and are deliberately
//! NOT part of the chain link: the digest change is only known once the
//! write's recompute is done, so an already-saved entry may be annotated
//! after its own save without breaking a successor's `prev` link, and
//! entries persisted before the fields existed keep verifying unchanged.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Most entries retained; older ones roll off the front, and the chain stays
/// verifiable from the oldest retained entry's `prev`.
pub const MAX_IDENTITY_AUDIT_ENTRIES: usize = 4_096;

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IdentityEvent {
    ApprovalKeyRegistered,
    ApprovalKeyRevoked,
    Initialized,
    ModeTransition,
    PolicyUpdated,
    UserCreated,
    UserUpdated,
    UserStatusChanged,
    UserUnlocked,
    PasswordSet,
    PasswordChanged,
    LoginSucceeded,
    LoginFailed,
    LoginThrottled,
    SessionRevoked,
    TokenIssued,
    TokenRedeemed,
    ApiKeyIssued,
    ApiKeyUsed,
    ApiKeyRevoked,
    MfaEnrolled,
    MfaVerified,
    MfaFailed,
    RecoveryCodesSet,
    RoleChanged,
    GroupChanged,
    IdpChanged,
    IdentityLinked,
    IdentityUnlinked,
    /// A `RegisterIdentity` outside the store, recorded in its audit trail.
    RbacIdentityRegistered,
    /// An `RbacAdmin` role or grant change, recorded in the store's audit trail.
    RbacPolicyChanged,
    /// A sampled authorization denial, drained into the durable audit trail.
    AccessDenied,
    /// An administrator's SQL dump was merged.
    Imported,
    /// Governed changes (proposals requiring two-person approval).
    ChangeProposed,
    ChangeApproved,
    ChangeRevoked,
    ChangeConsumed,
    /// Directory provisioning (SCIM / LDAP sync).
    UserProvisioned,
    UserDeprovisioned,
    DirectoryGroupChanged,
    /// A second factor was removed (WebAuthn credential).
    MfaRemoved,
    /// A signed-out user asked for a password reset (issued or not).
    PasswordResetRequested,
    /// An administrator registered or repaired the engine's System identity.
    SystemIdentityRepaired,
    /// A WorkItem delegation record was written or revoked.
    DelegationRecorded,
    DelegationRevoked,
}

/// The change of the canonical authorization digest across one identity or
/// RBAC write: the digest of the durable authorization image before the
/// write and after its recomputed projection. An empty change (the image
/// did not move) records equal values; an entry recorded without a recompute
/// in view leaves the whole field unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AuthorizationDigestChange {
    /// Digest of the authorization image before the write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Digest of the recomputed authorization image after the write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// One tamper-evident entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IdentityAuditEntry {
    pub seq: u64,
    pub at_ms: u64,
    /// The acting principal (or `engine` for a store-internal event).
    pub actor: String,
    pub event: IdentityEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_prefix: Option<String>,
    /// A short, secret-free detail (an op name, a reason code).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// `chain` of the previous entry (empty for the first ever).
    pub prev: String,
    /// sha256 over `prev` and every other field of this entry.
    pub chain: String,
    /// The tenant the change was made under; `None` for engine-wide events.
    /// Outside the chain link (see the module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// The authorization digest change the write produced, if one was
    /// recorded. Outside the chain link (see the module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_digest: Option<AuthorizationDigestChange>,
}

/// What a caller records; the trail assigns `seq`, `prev` and `chain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub at_ms: u64,
    pub actor: String,
    pub event: IdentityEvent,
    pub target: Option<String>,
    pub ip_prefix: Option<String>,
    pub detail: String,
    pub tenant: Option<String>,
    pub authorization_digest: Option<AuthorizationDigestChange>,
}

#[derive(Serialize)]
struct ChainBody<'a> {
    prev: &'a str,
    seq: u64,
    at_ms: u64,
    actor: &'a str,
    event: IdentityEvent,
    target: Option<&'a str>,
    ip_prefix: Option<&'a str>,
    detail: &'a str,
}

impl IdentityAuditEntry {
    fn chain_of(&self) -> String {
        let body = ChainBody {
            prev: &self.prev,
            seq: self.seq,
            at_ms: self.at_ms,
            actor: &self.actor,
            event: self.event,
            target: self.target.as_deref(),
            ip_prefix: self.ip_prefix.as_deref(),
            detail: &self.detail,
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(b"eg/identity-audit/v1\0");
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }
}

/// The bounded trail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditTrail {
    entries: std::collections::VecDeque<IdentityAuditEntry>,
    next_seq: u64,
}

impl AuditTrail {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> impl Iterator<Item = &IdentityAuditEntry> {
        self.entries.iter()
    }

    /// The seq of the newest retained entry, if any.
    pub fn last_seq(&self) -> Option<u64> {
        self.entries.back().map(|entry| entry.seq)
    }

    /// Name the tenant and the authorization digest change on every entry
    /// recorded after `after_seq` (on every entry when `None`). The fields
    /// are outside the chain link, so this never disturbs a `prev` or
    /// `chain` value and an already-saved entry may be annotated after its
    /// own save.
    pub fn annotate_since(
        &mut self,
        after_seq: Option<u64>,
        tenant: Option<&str>,
        authorization_digest: Option<AuthorizationDigestChange>,
    ) {
        let first = after_seq.map_or(0, |seq| seq + 1);
        for entry in self.entries.iter_mut() {
            if entry.seq >= first {
                if let Some(tenant) = tenant {
                    entry.tenant = Some(tenant.to_string());
                }
                if let Some(change) = &authorization_digest {
                    entry.authorization_digest = Some(change.clone());
                }
            }
        }
    }

    /// Append one record, rolling the oldest entry off past the bound.
    pub fn append(&mut self, record: AuditRecord) {
        let prev = self
            .entries
            .back()
            .map(|entry| entry.chain.clone())
            .unwrap_or_default();
        let mut entry = IdentityAuditEntry {
            seq: self.next_seq,
            at_ms: record.at_ms,
            actor: record.actor,
            event: record.event,
            target: record.target,
            ip_prefix: record.ip_prefix,
            detail: record.detail,
            prev,
            chain: String::new(),
            tenant: record.tenant,
            authorization_digest: record.authorization_digest,
        };
        entry.chain = entry.chain_of();
        self.next_seq += 1;
        self.entries.push_back(entry);
        while self.entries.len() > MAX_IDENTITY_AUDIT_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Verify every retained link. `Err(seq)` names the first broken entry.
    pub fn verify(&self) -> Result<(), u64> {
        let mut prev: Option<&str> = None;
        for entry in &self.entries {
            let linked = prev.is_none_or(|chain| chain == entry.prev);
            if !linked || entry.chain != entry.chain_of() {
                return Err(entry.seq);
            }
            prev = Some(&entry.chain);
        }
        Ok(())
    }

    /// Entries with `seq > after`, at most `limit`.
    pub fn page(&self, after: Option<u64>, limit: usize) -> Vec<IdentityAuditEntry> {
        self.entries
            .iter()
            .filter(|entry| after.is_none_or(|after| entry.seq > after))
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(event: IdentityEvent) -> AuditRecord {
        AuditRecord {
            at_ms: 1_800_000_000_000,
            actor: "svc:au".to_string(),
            event,
            target: Some("t".to_string()),
            ip_prefix: None,
            detail: String::new(),
            tenant: None,
            authorization_digest: None,
        }
    }

    // spec: MEDULLA-IDENTITY-R002
    #[test]
    fn entry_carries_tenant_and_the_authorization_digest_change() {
        let mut trail = AuditTrail::default();
        let change = AuthorizationDigestChange {
            from: Some("a".repeat(128)),
            to: Some("b".repeat(128)),
        };
        let before = trail.last_seq();
        trail.append(record(IdentityEvent::RbacPolicyChanged));
        trail.annotate_since(before, Some("tenant:homelab"), Some(change.clone()));
        let entry = trail.entries().last().unwrap();
        assert_eq!(entry.tenant.as_deref(), Some("tenant:homelab"));
        assert_eq!(entry.authorization_digest.as_ref(), Some(&change));
        assert!(trail.verify().is_ok());
    }

    // spec: MEDULLA-IDENTITY-R002
    #[test]
    fn annotate_since_leaves_older_entries_untouched() {
        let mut trail = AuditTrail::default();
        trail.append(record(IdentityEvent::UserCreated));
        trail.append(record(IdentityEvent::RoleChanged));
        trail.annotate_since(Some(0), Some("tenant:homelab"), None);
        let entries: Vec<_> = trail.entries().collect();
        assert!(entries[0].tenant.is_none());
        assert_eq!(entries[1].tenant.as_deref(), Some("tenant:homelab"));
        assert!(trail.verify().is_ok());
    }

    // spec: MEDULLA-IDENTITY-R002
    #[test]
    fn entries_serialized_before_the_r002_fields_still_deserialize_and_verify() {
        let mut trail = AuditTrail::default();
        trail.append(record(IdentityEvent::RbacIdentityRegistered));
        let wire = serde_json::to_string(trail.entries().next().unwrap()).unwrap();
        assert!(!wire.contains("\"tenant\""));
        assert!(!wire.contains("\"authorization_digest\""));
        // A stored entry in the pre-R002 shape deserializes with the new
        // fields unset and keeps its chain, so an upgraded store verifies.
        let legacy: IdentityAuditEntry = serde_json::from_str(&wire).unwrap();
        assert!(legacy.tenant.is_none());
        assert!(legacy.authorization_digest.is_none());
        assert_eq!(legacy.chain, legacy.chain_of());
    }
}
