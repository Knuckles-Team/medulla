//! Credential records, requests and views of the engine identity store added by spec pass 14
//! (MEDULLA-IDENTITY-R730 to R735): administrator API keys, session revoke by handle, the
//! issuer key ring, SCIM client credentials, administrator reset tokens and the MFA challenge
//! store.
//!
//! No view carries a secret or a secret hash. A secret is hashed at the request boundary and
//! only the hash is stored; a challenge is public to the client that holds it, so it is stored
//! as given.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::stamp::Secret;

/// The most issuer keys the ring holds, active and retired together.
pub const MAX_ISSUER_KEYS: usize = 16;
/// How long a retired issuer key stays in the ring so tokens it signed still verify.
pub const ISSUER_KEY_GRACE_MS: u64 = 24 * 60 * 60 * 1_000;
/// The most SCIM client credentials one store holds.
pub const MAX_SCIM_CLIENTS: usize = 64;
/// The most live MFA challenges one store holds.
pub const MAX_MFA_CHALLENGES: usize = 4_096;
/// How long an MFA challenge stays valid: five minutes.
pub const MFA_CHALLENGE_TTL_MS: u64 = 300_000;
/// Length of a session handle (hex characters).
pub const SESSION_HANDLE_CHARS: usize = 12;

/// The signing algorithms an issuer key may declare.
pub const ISSUER_KEY_ALGORITHMS: [&str; 2] = ["EdDSA", "ES256"];

/// One public key of the issuer ring. The private half never reaches the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerKeyRecord {
    pub kid: String,
    pub algorithm: String,
    pub public_key_hex: String,
    pub created_at_ms: u64,
    /// Set when a newer key replaced this one; the key stays published for
    /// [`ISSUER_KEY_GRACE_MS`] after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at_ms: Option<u64>,
}

impl IssuerKeyRecord {
    /// Whether the key is still published at `now_ms`: active, or retired inside the grace.
    pub fn published(&self, now_ms: u64) -> bool {
        self.retired_at_ms
            .is_none_or(|retired| retired.saturating_add(ISSUER_KEY_GRACE_MS) > now_ms)
    }
}

/// One SCIM client credential: the client authenticates with a secret whose hash is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScimClientRecord {
    pub client_id: String,
    pub label: String,
    pub secret_hash: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

/// What an MFA challenge is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MfaChallengeKind {
    /// A TOTP or WebAuthn enrollment in progress.
    Enrollment,
    /// A WebAuthn assertion to be verified.
    Webauthn,
}

/// One stored challenge. It lives until its expiry and is spent exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MfaChallengeRecord {
    pub principal_id: String,
    pub kind: MfaChallengeKind,
    pub challenge: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

/// `issue_api_key_admin`: an administrator's direct issue, with no session token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ApiKeyAdminIssue {
    pub principal_id: String,
    pub key_id: String,
    pub secret: Secret,
    pub scopes: BTreeSet<String>,
    pub ttl_ms: u64,
}

/// `revoke_session_by_handle`: the handle a session listing shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SessionHandleRef {
    pub handle: String,
}

/// `rotate_issuer_key`: register a new public key as the active one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IssuerKeyRotate {
    pub kid: String,
    pub algorithm: String,
    pub public_key_hex: String,
}

/// `create_scim_client`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScimClientCreate {
    pub client_id: String,
    pub label: String,
    pub secret: Secret,
    pub ttl_ms: u64,
}

/// `verify_scim_client`: the client id and the secret it presents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScimClientUse {
    pub client_id: String,
    pub secret: Secret,
}

/// `update_scim_client`: each present field replaces the stored one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScimClientUpdate {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A new secret; the old one stops working at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<Secret>,
    /// A new lifetime from now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// `issue_admin_reset`: an administrator issues a one-time reset token for a principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdminResetIssue {
    pub principal_id: String,
    pub token: Secret,
    pub ttl_ms: u64,
}

/// `issue_mfa_challenge`: the session's principal gets one challenge of `kind`; a new one
/// replaces the outstanding one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MfaChallengeIssue {
    pub session_token: Secret,
    pub kind: MfaChallengeKind,
    pub challenge: String,
}

/// `consume_mfa_challenge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MfaChallengeConsume {
    pub session_token: Secret,
    pub kind: MfaChallengeKind,
}

/// One API key as a listing shows it: never the secret or its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ApiKeyView {
    pub key_id: String,
    pub principal_id: String,
    pub scopes: BTreeSet<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u64>,
}

/// One published issuer key: the public half and its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IssuerKeyView {
    pub kid: String,
    pub algorithm: String,
    pub public_key_hex: String,
    pub created_at_ms: u64,
    /// `active` or `retired`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at_ms: Option<u64>,
}

/// One SCIM client as a listing shows it: never the secret or its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScimClientView {
    pub client_id: String,
    pub label: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

/// A spent MFA challenge, answered once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MfaChallengeView {
    pub kind: MfaChallengeKind,
    pub challenge: String,
    pub expires_at_ms: u64,
}
