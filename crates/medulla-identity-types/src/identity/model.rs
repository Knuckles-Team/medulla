//! Principal and credential records (§3.2). Hash and sealed-secret fields are
//! never serialized into a view; see `views.rs` for what a reader sees.

use std::collections::BTreeSet;

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// Whether a principal is a person or a workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UserKind {
    Human,
    Service,
}

/// A principal's lifecycle state. Nothing is ever deleted: a deprovisioned
/// principal keeps its id, links and owned data, and holds no authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UserStatus {
    Active,
    Disabled,
    /// Must set a new password (admin reset) before any session is issued.
    PendingReset,
    PendingVerification,
    /// Holds no authority. The directory can re-provision it (edge U13).
    Deprovisioned,
}

impl UserStatus {
    /// Only an active principal holds authority or may sign in.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

// MEDULLA-IDENTITY-R721.1 — the user status edge table and its one transition function.
// UserStatusMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// An event of `UserStatusMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserStatusEvent {
    /// The `Disable` event.
    Disable,
    /// The `Enable` event.
    Enable,
    /// The `RequireReset` event.
    RequireReset,
    /// The `CompleteReset` event.
    CompleteReset,
    /// The `Verify` event.
    Verify,
    /// The `Deprovision` event.
    Deprovision,
    /// The `Reprovision` event.
    Reprovision,
}

impl MachineState for UserStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::PendingReset => "pending_reset",
            Self::PendingVerification => "pending_verification",
            Self::Deprovisioned => "deprovisioned",
        }
    }
}

impl MachineEvent for UserStatusEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Disable => "Disable",
            Self::Enable => "Enable",
            Self::RequireReset => "RequireReset",
            Self::CompleteReset => "CompleteReset",
            Self::Verify => "Verify",
            Self::Deprovision => "Deprovision",
            Self::Reprovision => "Reprovision",
        }
    }
}

/// Guard of `UserStatusMachine`: the principal is not the last active administrator.
pub const USER_STATUS_GUARD_NOT_LAST_ACTIVE_ADMINISTRATOR: &str = "NotLastActiveAdministrator";
/// Guard of `UserStatusMachine`: the new credential is set.
pub const USER_STATUS_GUARD_NEW_CREDENTIAL_SET: &str = "NewCredentialSet";
/// Guard of `UserStatusMachine`: the verification proof matches.
pub const USER_STATUS_GUARD_VERIFICATION_PROOF_MATCHES: &str = "VerificationProofMatches";
/// Guard of `UserStatusMachine`: the directory that provisioned the principal re-provisions it.
pub const USER_STATUS_GUARD_DIRECTORY_REPROVISION: &str = "DirectoryReprovision";

/// `UserStatusMachine`: the closed edge table. Every other pair of state and event is refused.
pub static USER_STATUS_MACHINE: MachineDef<UserStatus, UserStatusEvent, &'static str> =
    MachineDef {
        name: "UserStatusMachine",
        states: &[
            UserStatus::Active,
            UserStatus::Disabled,
            UserStatus::PendingReset,
            UserStatus::PendingVerification,
            UserStatus::Deprovisioned,
        ],
        initial: UserStatus::Active,
        terminals: &[],
        events: &[
            UserStatusEvent::Disable,
            UserStatusEvent::Enable,
            UserStatusEvent::RequireReset,
            UserStatusEvent::CompleteReset,
            UserStatusEvent::Verify,
            UserStatusEvent::Deprovision,
            UserStatusEvent::Reprovision,
        ],
        guards: &[
            USER_STATUS_GUARD_NOT_LAST_ACTIVE_ADMINISTRATOR,
            USER_STATUS_GUARD_NEW_CREDENTIAL_SET,
            USER_STATUS_GUARD_VERIFICATION_PROOF_MATCHES,
            USER_STATUS_GUARD_DIRECTORY_REPROVISION,
        ],
        edges: &[
            Edge {
                id: "U01",
                from: UserStatus::Active,
                event: UserStatusEvent::Disable,
                guard: Some(USER_STATUS_GUARD_NOT_LAST_ACTIVE_ADMINISTRATOR),
                to: UserStatus::Disabled,
            },
            Edge {
                id: "U02",
                from: UserStatus::Disabled,
                event: UserStatusEvent::Enable,
                guard: None,
                to: UserStatus::Active,
            },
            Edge {
                id: "U03",
                from: UserStatus::Active,
                event: UserStatusEvent::RequireReset,
                guard: Some(USER_STATUS_GUARD_NOT_LAST_ACTIVE_ADMINISTRATOR),
                to: UserStatus::PendingReset,
            },
            Edge {
                id: "U04",
                from: UserStatus::Disabled,
                event: UserStatusEvent::RequireReset,
                guard: None,
                to: UserStatus::PendingReset,
            },
            Edge {
                id: "U05",
                from: UserStatus::PendingReset,
                event: UserStatusEvent::CompleteReset,
                guard: Some(USER_STATUS_GUARD_NEW_CREDENTIAL_SET),
                to: UserStatus::Active,
            },
            Edge {
                id: "U06",
                from: UserStatus::PendingReset,
                event: UserStatusEvent::Disable,
                guard: None,
                to: UserStatus::Disabled,
            },
            Edge {
                id: "U07",
                from: UserStatus::PendingVerification,
                event: UserStatusEvent::Verify,
                guard: Some(USER_STATUS_GUARD_VERIFICATION_PROOF_MATCHES),
                to: UserStatus::Active,
            },
            Edge {
                id: "U08",
                from: UserStatus::PendingVerification,
                event: UserStatusEvent::Disable,
                guard: None,
                to: UserStatus::Disabled,
            },
            Edge {
                id: "U09",
                from: UserStatus::Active,
                event: UserStatusEvent::Deprovision,
                guard: Some(USER_STATUS_GUARD_NOT_LAST_ACTIVE_ADMINISTRATOR),
                to: UserStatus::Deprovisioned,
            },
            Edge {
                id: "U10",
                from: UserStatus::Disabled,
                event: UserStatusEvent::Deprovision,
                guard: None,
                to: UserStatus::Deprovisioned,
            },
            Edge {
                id: "U11",
                from: UserStatus::PendingReset,
                event: UserStatusEvent::Deprovision,
                guard: None,
                to: UserStatus::Deprovisioned,
            },
            Edge {
                id: "U12",
                from: UserStatus::PendingVerification,
                event: UserStatusEvent::Deprovision,
                guard: None,
                to: UserStatus::Deprovisioned,
            },
            Edge {
                id: "U13",
                from: UserStatus::Deprovisioned,
                event: UserStatusEvent::Reprovision,
                guard: Some(USER_STATUS_GUARD_DIRECTORY_REPROVISION),
                to: UserStatus::Active,
            },
        ],
    };

impl UserStatus {
    /// The one transition function of `UserStatusMachine`. `guard_holds` answers each guard
    /// by name; an unguarded edge always holds.
    pub fn apply(
        self,
        event: UserStatusEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&USER_STATUS_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }

    /// The event of the one operator-driven edge from `self` to `to`, if there is one. The
    /// directory edge (`Reprovision`) is never chosen by target: only the directory fires it.
    pub fn event_to(self, to: Self) -> Option<UserStatusEvent> {
        USER_STATUS_MACHINE
            .edges_from(self)
            .find(|edge| edge.to == to && edge.event != UserStatusEvent::Reprovision)
            .map(|edge| edge.event)
    }
}

/// One principal. `principal_id` is opaque, stable and never renamed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserRecord {
    pub principal_id: String,
    /// Normalized (see `normalize_username`), unique, mutable.
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub kind: UserKind,
    pub status: UserStatus,
    /// The built-in demo/bootstrap principal (`usr:bootstrap`).
    #[serde(default)]
    pub is_bootstrap: bool,
    /// `local`, `scim:<idp>`, `ldap:<idp>` or `jit:<idp>`.
    pub source: String,
    /// Roles granted directly (group roles are resolved through groups).
    #[serde(default)]
    pub roles: BTreeSet<String>,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_login_at_ms: Option<u64>,
}

/// A password credential: an argon2id PHC string computed by the engine at
/// the request boundary. The candidate never reaches the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordCredential {
    pub hash: String,
    pub changed_at_ms: u64,
    #[serde(default)]
    pub must_change: bool,
    /// Previous hashes, newest first, for the reuse check.
    #[serde(default)]
    pub history: Vec<String>,
    /// Which password this is. It moves whenever the password is replaced
    /// and whenever the principal's sessions are revoked; a rehash of the
    /// same password keeps it. The boundary's verdict names the generation
    /// it verified against, and apply refuses a verdict for any other. A
    /// record written before the counter existed carries none and reads as
    /// generation 0.
    #[serde(default)]
    pub generation: u64,
}

/// A server-side session. Only the SHA-256 of the session id is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub session_hash: String,
    /// Effective scopes at session creation. Missing legacy evidence cannot authorize renewal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_scopes: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tenant: Option<String>,
    pub principal_id: String,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub idle_expires_at_ms: u64,
    pub absolute_expires_at_ms: u64,
    /// `pwd`, `totp`, `recovery`, `idp:<id>`.
    pub auth_methods: Vec<String>,
    /// The session still owes a second factor; it authorizes nothing until
    /// `verify_totp` / `consume_recovery_code` completes it.
    #[serde(default)]
    pub mfa_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_at_ms: Option<u64>,
    /// Truncated client address (/24 v4, /48 v6) for audit only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke_reason: Option<String>,
}

impl SessionRecord {
    /// Live = not revoked, inside both expiry bounds, and second factor done.
    pub fn is_live(&self, now_ms: u64) -> bool {
        self.revoked_at_ms.is_none()
            && now_ms < self.idle_expires_at_ms
            && now_ms < self.absolute_expires_at_ms
    }
}

/// What a one-time token is for. A token redeems only for its purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TokenPurpose {
    CredentialReset,
    EmailVerify,
    Invite,
    LinkClaim,
    AdminReset,
}

/// A single-use token. Only its hash is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneTimeToken {
    pub token_hash: String,
    pub purpose: TokenPurpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_at_ms: Option<u64>,
    pub created_by: String,
}

/// An API key. `key_id` is public (`gok_<id>`); only the secret's hash is
/// stored. Its scopes are a subset of its owner's, re-checked at every use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub principal_id: String,
    pub secret_hash: String,
    pub scopes: BTreeSet<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u64>,
}

/// A TOTP factor. The shared secret is sealed by the engine's data key at
/// the request boundary; the store never holds it in the clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpRecord {
    pub sealed_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at_ms: Option<u64>,
    /// Highest accepted RFC 6238 time step (replay guard).
    #[serde(default)]
    pub last_step: u64,
}

/// A WebAuthn (passkey / security key) credential. graph-os verifies every
/// attestation and assertion signature with a vetted library; the store
/// keeps the PUBLIC credential and enforces the signature counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebauthnRecord {
    pub credential_id: String,
    pub principal_id: String,
    /// The COSE public key, base64url.
    pub public_key_cose: String,
    pub sign_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aaguid: Option<String>,
    #[serde(default)]
    pub transports: Vec<String>,
    pub name: String,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<u64>,
}

/// A link from an identity-provider subject to a principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExternalIdentity {
    pub idp_id: String,
    pub subject: String,
    pub principal_id: String,
    pub linked_at_ms: u64,
    pub linked_by: String,
}
