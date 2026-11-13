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

/// How a wire login proves itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        }
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
}
