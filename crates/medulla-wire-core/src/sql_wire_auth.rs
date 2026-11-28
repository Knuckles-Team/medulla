//! Shared configuration and credential primitives for the SQL wire adapters.
//!
//! Protocol modules own their wire-specific authentication exchanges. This
//! module owns only the policy that is identical at that boundary: a mode is
//! resolved after key material is checked, and user credentials are derived
//! with a domain-separated HMAC.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Resolve a fail-closed SQL-wire mode while preserving the adapter's public
/// error wording and validation order.
pub fn resolve_mode(
    auth_secret: &str,
    env_var: &str,
    expected: &str,
    missing_secret_error: &'static str,
    invalid_mode_error: &'static str,
) -> std::io::Result<()> {
    if auth_secret.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            missing_secret_error,
        ));
    }
    match std::env::var(env_var) {
        Err(std::env::VarError::NotPresent) => Ok(()),
        Ok(value) if value.trim().eq_ignore_ascii_case(expected) => Ok(()),
        Ok(_) | Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            invalid_mode_error,
        )),
    }
}

/// Derive a fixed-size domain-separated credential digest.
pub fn derive_hmac_digest(secret: &str, domain: &[u8], principal: &str) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(domain);
    mac.update(principal.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

/// Derive the public hex form used by the SQL-wire password conventions.
pub fn derive_hmac_hex(secret: &str, domain: &[u8], principal: &str) -> String {
    hex::encode(derive_hmac_digest(secret, domain, principal))
}

// ---------------------------------------------------------------------------
// Wire logins (MEDULLA-ENGINE-PARITY-R007.1): a login resolves to one principal.
// ---------------------------------------------------------------------------

use medulla_kernel::lock_recovery::{LockRecovery, WriteRecovery};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use {
    medulla_identity_core_types::identity::WireLoginKind,
    medulla_identity_core_types::identity::WireLoginRecord,
};

/// Stable refusal code of an unknown, revoked or mismatched wire login.
pub const INVALID_LOGIN: &str = "INVALID_LOGIN";
/// SQLSTATE of [`INVALID_LOGIN`].
pub const INVALID_LOGIN_SQLSTATE: &str = "28000";

/// Where wire-login records are read from.
pub trait WireLoginSource: Send + Sync {
    fn find(&self, login: &str) -> Option<WireLoginRecord>;
}

/// What the connection presented for the login.
#[derive(Clone, Copy, Debug)]
pub enum WireProof<'a> {
    /// The SCRAM exchange against the record's verifier already succeeded.
    ScramVerified,
    /// The DER of the client certificate the mTLS handshake accepted.
    Certificate { certificate_der: &'a [u8] },
    /// The token carried in the password message.
    Token(&'a str),
}

impl WireProof<'_> {
    fn kind(&self) -> WireLoginKind {
        match self {
            Self::ScramVerified => WireLoginKind::Password,
            Self::Certificate { .. } => WireLoginKind::Certificate,
            Self::Token(_) => WireLoginKind::Token,
        }
    }
}

/// The identity a successful login resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedWireLogin {
    pub login: String,
    pub kind: WireLoginKind,
    pub principal: String,
    pub tenant: String,
}

/// `INVALID_LOGIN` (SQLSTATE `28000`); the reason never says which check failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidLogin {
    pub login: String,
}

impl std::fmt::Display for InvalidLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{INVALID_LOGIN}: login `{}` is not valid", self.login)
    }
}

impl std::error::Error for InvalidLogin {}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Resolve `login` with the presented `proof`: look the record up, refuse a
/// revoked or kind-mismatched record, and check the kind's verifier.
pub fn resolve_wire_login(
    source: &dyn WireLoginSource,
    login: &str,
    proof: WireProof<'_>,
) -> Result<ResolvedWireLogin, InvalidLogin> {
    let invalid = || InvalidLogin {
        login: login.to_string(),
    };
    let record = source.find(login).ok_or_else(invalid)?;
    if !record.is_active() || record.kind != proof.kind() {
        return Err(invalid());
    }
    let verified = match proof {
        WireProof::ScramVerified => true,
        WireProof::Certificate { certificate_der } => constant_time_eq(
            medulla_identity_core_types::identity::sha256_verifier(certificate_der).as_bytes(),
            record.verifier.as_bytes(),
        ),
        WireProof::Token(token) => constant_time_eq(
            medulla_identity_core_types::identity::sha256_verifier(token.as_bytes()).as_bytes(),
            record.verifier.as_bytes(),
        ),
    };
    if !verified {
        return Err(invalid());
    }
    Ok(ResolvedWireLogin {
        login: record.login,
        kind: record.kind,
        principal: record.principal,
        tenant: record.tenant,
    })
}

/// A source that keeps each lookup for at most `ttl` (knob
/// `wire_login.cache_ttl_s`); a zero ttl never caches.
pub struct CachedWireLogins {
    inner: Arc<dyn WireLoginSource>,
    ttl: Duration,
    cache: RwLock<HashMap<String, (Instant, Option<WireLoginRecord>)>>,
}

impl CachedWireLogins {
    pub fn new(inner: Arc<dyn WireLoginSource>, ttl_s: u64) -> Self {
        Self {
            inner,
            ttl: Duration::from_secs(ttl_s),
            cache: RwLock::new(HashMap::new()),
        }
    }
}

impl WireLoginSource for CachedWireLogins {
    fn find(&self, login: &str) -> Option<WireLoginRecord> {
        if self.ttl.is_zero() {
            return self.inner.find(login);
        }
        if let Some((at, hit)) = self.cache.lock_recovering("wire login cache").get(login) {
            if at.elapsed() < self.ttl {
                return hit.clone();
            }
        }
        let found = self.inner.find(login);
        self.cache
            .write_recovering("wire login cache")
            .insert(login.to_string(), (Instant::now(), found.clone()));
        found
    }
}
