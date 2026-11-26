//! Wire logins: the record that maps one SQL-wire login to one principal
//! (MEDULLA-ENGINE-PARITY-R007.1).
//!
//! A login authenticates in one of three kinds. The `verifier` is never a
//! plaintext secret: it is the SCRAM salted credential for `Password`, the
//! SHA-256 of the client certificate public key for `Certificate`, and the
//! SHA-256 of the token for `Token`.
//!
//! The record's lifecycle is the explicit machine `wire_login`:
//! `Active --Revoke--> Revoked`. [`WireLoginRecord::apply`] is the only writer
//! of `state`; `revoked_ms` is the data the revoke transition writes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The scopes a wire login holds when its grant names none: plain graph read and write.
pub const DEFAULT_WIRE_LOGIN_SCOPES: [&str; 2] = ["kg:read", "kg:write"];

/// The scope an administrator grants to let a login open a replication connection.
pub const REPLICATION_READ_SCOPE: &str = "replication:read";

/// The most scopes one wire login carries.
pub const MAX_WIRE_LOGIN_SCOPES: usize = 16;

/// [`DEFAULT_WIRE_LOGIN_SCOPES`] as owned strings; the serde default of a record or a grant that
/// names no scopes.
pub fn default_wire_login_scopes() -> Vec<String> {
    DEFAULT_WIRE_LOGIN_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
}

/// How a wire login proves itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WireLoginKind {
    Password,
    Certificate,
    Token,
}

impl WireLoginKind {
    pub const ALL: [WireLoginKind; 3] = [Self::Password, Self::Certificate, Self::Token];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Certificate => "certificate",
            Self::Token => "token",
        }
    }
}

/// The closed set of lifecycle states of a wire login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireLoginState {
    Active,
    Revoked,
}

impl WireLoginState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

/// The closed set of events of the `wire_login` machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireLoginEvent {
    /// Revoke the login at `now_ms`.
    Revoke { now_ms: u64 },
}

impl WireLoginEvent {
    pub fn name(self) -> &'static str {
        match self {
            Self::Revoke { .. } => "Revoke",
        }
    }
}

/// An event the machine refuses in the current state.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("ILLEGAL_TRANSITION: machine wire_login in state {state} refuses event {event}")]
pub struct WireLoginError {
    pub state: &'static str,
    pub event: &'static str,
}

/// One wire login.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireLoginRecord {
    pub login: String,
    pub kind: WireLoginKind,
    /// Hex verifier; see the module docs for what each kind holds.
    pub verifier: String,
    pub principal: String,
    pub tenant: String,
    pub created_ms: u64,
    pub state: WireLoginState,
    /// When the revoke transition ran; `None` while `Active`.
    pub revoked_ms: Option<u64>,
    /// The scopes the login's connection carries: `kg:read` and `kg:write` unless the grant names
    /// others. A walsender connection needs `replication:read` here. A record stored before the
    /// field existed reads as the default.
    #[serde(default = "default_wire_login_scopes")]
    pub scopes: Vec<String>,
}

impl WireLoginRecord {
    /// A new `Active` record.
    pub fn new(
        login: &str,
        kind: WireLoginKind,
        verifier: &str,
        principal: &str,
        tenant: &str,
        created_ms: u64,
    ) -> Self {
        Self {
            login: login.to_string(),
            kind,
            verifier: verifier.to_string(),
            principal: principal.to_string(),
            tenant: tenant.to_string(),
            created_ms,
            state: WireLoginState::Active,
            revoked_ms: None,
            scopes: default_wire_login_scopes(),
        }
    }

    /// The record with `scopes` in place of the default.
    pub fn with_scopes(mut self, scopes: Vec<String>) -> Self {
        self.scopes = scopes;
        self
    }

    /// The single transition function: the only writer of `state`.
    pub fn apply(&mut self, event: WireLoginEvent) -> Result<(), WireLoginError> {
        match (self.state, event) {
            (WireLoginState::Active, WireLoginEvent::Revoke { now_ms }) => {
                self.state = WireLoginState::Revoked;
                self.revoked_ms = Some(now_ms);
                Ok(())
            }
            (state, event) => Err(WireLoginError {
                state: state.as_str(),
                event: event.name(),
            }),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state == WireLoginState::Active
    }
}

/// What `grant_wire_login` answers (MEDULLA-ENGINE-PARITY-R007.4): the stored record's public
/// facts. The verifier is never echoed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WireLoginGranted {
    pub login: String,
    pub kind: WireLoginKind,
    pub principal: String,
    pub tenant: String,
    pub created_ms: u64,
    pub scopes: Vec<String>,
}

impl From<&WireLoginRecord> for WireLoginGranted {
    fn from(record: &WireLoginRecord) -> Self {
        Self {
            login: record.login.clone(),
            kind: record.kind,
            principal: record.principal.clone(),
            tenant: record.tenant.clone(),
            created_ms: record.created_ms,
            scopes: record.scopes.clone(),
        }
    }
}

/// What `revoke_wire_login` answers (MEDULLA-ENGINE-PARITY-R007.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WireLoginRevoked {
    pub login: String,
    pub revoked_ms: u64,
}

/// Whether `verifier` is in the form its `kind` stores (MEDULLA-ENGINE-PARITY-R007.4): the SCRAM
/// salted credential `<salt hex>:<salted password hex>` for `Password`, the 64-character
/// lowercase [`sha256_verifier`] hex for `Certificate` and `Token`.
pub fn verifier_is_well_formed(kind: WireLoginKind, verifier: &str) -> bool {
    let lower_hex = |text: &str| {
        !text.is_empty()
            && text.len() % 2 == 0
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    match kind {
        WireLoginKind::Password => verifier
            .split_once(':')
            .is_some_and(|(salt, salted)| lower_hex(salt) && lower_hex(salted)),
        WireLoginKind::Certificate | WireLoginKind::Token => {
            verifier.len() == 64 && lower_hex(verifier)
        }
    }
}

/// Hex SHA-256: the verifier of a `Certificate` (over the public key) or a
/// `Token` (over the token).
pub fn sha256_verifier(material: &[u8]) -> String {
    hex::encode(Sha256::digest(material))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> WireLoginRecord {
        WireLoginRecord::new("bi", WireLoginKind::Token, "ab", "agent:bi", "t1", 10)
    }

    // spec: MEDULLA-ENGINE-PARITY-R007.1
    #[test]
    fn revoke_is_the_only_legal_transition_and_keeps_its_time() {
        let mut r = record();
        assert!(r.is_active() && r.revoked_ms.is_none());
        r.apply(WireLoginEvent::Revoke { now_ms: 99 })
            .expect("revoke");
        assert_eq!(r.state, WireLoginState::Revoked);
        assert_eq!(r.revoked_ms, Some(99));
        let err = r.apply(WireLoginEvent::Revoke { now_ms: 100 }).unwrap_err();
        assert_eq!(err.state, "revoked");
        assert_eq!(err.event, "Revoke");
        assert!(err.to_string().contains("wire_login"));
        assert_eq!(r.revoked_ms, Some(99), "a refused event changes nothing");
    }

    // spec: MEDULLA-ENGINE-PARITY-R007.1
    #[test]
    fn a_record_defaults_to_graph_read_and_write_and_old_records_decode_so() {
        let r = record();
        assert_eq!(
            r.scopes,
            vec!["kg:read".to_string(), "kg:write".to_string()]
        );
        let mut json = serde_json::to_value(&r).unwrap();
        json.as_object_mut().unwrap().remove("scopes");
        let old: WireLoginRecord = serde_json::from_value(json).unwrap();
        assert_eq!(old.scopes, r.scopes);
        let replicator = record().with_scopes(vec![REPLICATION_READ_SCOPE.to_string()]);
        assert_eq!(
            WireLoginGranted::from(&replicator).scopes,
            replicator.scopes
        );
    }
}
