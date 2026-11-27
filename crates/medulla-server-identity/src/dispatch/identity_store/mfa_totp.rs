//! The served reply of the engine-held TOTP ops (MEDULLA-IDENTITY-R741).
//!
//! The store applies the op and answers an `Authenticate` result (or `Done` for `enroll`): a
//! wrong code or a running backoff is a COMMITTED outcome, because the failure count it adds
//! must be durable, and the engine keeps an applied op's state only on success. This boundary
//! turns the outcome into the wire answer after the apply: a wrong code refuses
//! `MFA_CODE_INVALID`, a running backoff refuses `MFA_LOCKED: retry_at_ms=<n>`, and `enroll`
//! answers the provisioning URI built from the sealed record, once, here. The plaintext secret
//! is never stored, replicated or logged.

use medulla_identity_types::acl::RequestContextClaims;
use medulla_identity_types::{
    identity::AuthenticateOutcome, identity::IdentityOp, identity::IdentityRefusal,
    identity::IdentityReply, identity::IdentityStamp, identity::LoginStateReleased,
    identity::MfaOp, identity::MfaProvisioning, identity::TokenOp,
};
use medulla_server_api::state::ServerState;

use super::secrets;

/// The label an authenticator app shows for the account.
pub const ISSUER: &str = "medulla";

type Outcome = Result<IdentityReply<RequestContextClaims>, String>;

/// Turn the store's reply of a TOTP op into its served answer; every other op passes through.
/// Called with the server lock held, after the apply, with the apply's `now_ms`.
pub fn finish(
    state: &ServerState,
    op: &IdentityOp,
    stamp: &IdentityStamp,
    now_ms: u64,
    outcome: Outcome,
) -> Outcome {
    if let IdentityOp::Token(TokenOp::ConsumeLoginState { .. }) = op {
        return match outcome {
            Ok(IdentityReply::LoginStateSealed { sealed }) => {
                release_login_state(state, &sealed).map_err(|refusal| refusal.to_string())
            }
            other => other,
        };
    }
    let IdentityOp::Mfa(mfa) = op else {
        return outcome;
    };
    match (mfa, outcome) {
        (MfaOp::Enroll { .. }, Ok(IdentityReply::Done { .. })) => {
            provisioning(state, stamp, now_ms).map_err(|refusal| refusal.to_string())
        }
        (
            MfaOp::Confirm { .. } | MfaOp::Verify { .. } | MfaOp::Disable { .. },
            Ok(IdentityReply::Authenticate(result)),
        ) => match result.outcome {
            AuthenticateOutcome::Ok => Ok(IdentityReply::Authenticate(result)),
            AuthenticateOutcome::Throttled => Err(format!(
                "{}: retry_at_ms={}",
                IdentityRefusal::MfaLocked,
                now_ms.saturating_add(result.retry_after_ms.unwrap_or(0))
            )),
            _ => Err(IdentityRefusal::MfaCodeInvalid.to_string()),
        },
        (_, outcome) => outcome,
    }
}

/// The provisioning URI of the unconfirmed secret just stored for the enrolling session.
fn provisioning(
    state: &ServerState,
    stamp: &IdentityStamp,
    now_ms: u64,
) -> Result<IdentityReply<RequestContextClaims>, IdentityRefusal> {
    let store = state.isolation.rbac().identity_store();
    let hash = stamp.token_hash(0)?;
    let principal = store
        .session_principal(hash, now_ms)
        .ok_or(IdentityRefusal::NotAuthorized)?;
    let sealed = store
        .sealed_totp_of(principal)
        .ok_or(IdentityRefusal::MfaEnrollmentRequired)?;
    let secret =
        secrets::unseal(sealed, &state.auth_secret).map_err(|_| IdentityRefusal::Unstamped)?;
    let secret = String::from_utf8(secret).map_err(|_| IdentityRefusal::Unstamped)?;
    let account = store
        .user(principal)
        .map_or(principal, |user| user.username.as_str());
    Ok(IdentityReply::MfaProvisioning(MfaProvisioning {
        provisioning_uri: secrets::provisioning_uri(&secret, ISSUER, account),
    }))
}

/// The nonce and PKCE verifier of a consumed login state, unsealed once here and never stored
/// (MEDULLA-IDENTITY-R750).
fn release_login_state(
    state: &ServerState,
    sealed: &str,
) -> Result<IdentityReply<RequestContextClaims>, IdentityRefusal> {
    let payload =
        secrets::unseal(sealed, &state.auth_secret).map_err(|_| IdentityRefusal::Unstamped)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| IdentityRefusal::Unstamped)?;
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or(IdentityRefusal::Unstamped)
    };
    Ok(IdentityReply::LoginState(LoginStateReleased {
        nonce: text("nonce")?,
        pkce_verifier: text("pkce_verifier")?,
    }))
}
