//! Explicit local anonymous authority and unsigned replay metadata.
//!
//! The legacy `auth_token` wire slot is an envelope slot. `anon1.` contains
//! correlation metadata only, never a credential or proof of identity. Only
//! ServerState's explicit isolated anonymous mode calls this parser.
use super::authority_context::VerifiedRequestContext;
use medulla_protocol_types::protocol::Request;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnonymousOperation {
    idempotency_key: String,
}

/// Largest anonymous correlation metadata accepted after the `anon1.` prefix
/// (MEDULLA-IDENTITY-R003.2): 2 KiB.
pub const MAX_METADATA_BYTES: usize = 2048;
/// Largest anonymous correlation (idempotency) key in UTF-8 bytes.
pub const MAX_CORRELATION_KEY_BYTES: usize = 256;
/// The typed refusal for metadata past either bound; the request boundary
/// passes it through unchanged (MEDULLA-IDENTITY-R003.2).
pub const OVERSIZE_REFUSAL: &str =
    "ACCESS_DENIED: anonymous correlation metadata exceeds its 2 KiB or 256-byte key bound";
/// Prefix of every anonymous-mode refusal. Isolated mode needs no credential, so a
/// presented credential or identity claim is denied, not "authentication required";
/// the request boundary passes these refusals through unchanged (MEDULLA-IDENTITY-R003.3).
pub const REFUSAL_PREFIX: &str = "ACCESS_DENIED: anonymous ";

pub fn request_authority(request: &Request) -> Result<VerifiedRequestContext, String> {
    if request.agent_id.is_some() {
        return Err("ACCESS_DENIED: anonymous mode refuses caller identity claims".into());
    }
    if request.auth_token.is_empty() {
        return Ok(VerifiedRequestContext::anonymous_local(None));
    }
    let raw = request.auth_token.strip_prefix("anon1.").ok_or(
        "ACCESS_DENIED: anonymous mode refuses credentials and malformed correlation envelopes",
    )?;
    if raw.len() > MAX_METADATA_BYTES {
        return Err(OVERSIZE_REFUSAL.into());
    }
    let operation: AnonymousOperation = serde_json::from_str(raw).map_err(|_| {
        "ACCESS_DENIED: anonymous correlation metadata must contain only idempotency_key"
    })?;
    if operation.idempotency_key.len() > MAX_CORRELATION_KEY_BYTES {
        return Err(OVERSIZE_REFUSAL.into());
    }
    if operation.idempotency_key.trim().is_empty()
        || operation.idempotency_key.chars().any(char::is_control)
    {
        return Err(
            "ACCESS_DENIED: anonymous idempotency_key must be 1..256 UTF-8 bytes without controls"
                .into(),
        );
    }
    Ok(VerifiedRequestContext::anonymous_local(Some(
        operation.idempotency_key,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(token: &str) -> Request {
        Request {
            id: 1,
            graph: "__commons__".into(),
            agent_id: None,
            auth_token: token.into(),
            method: medulla_protocol_types::protocol::Method::Ping,
        }
    }
    // spec: MEDULLA-IDENTITY-R003.1
    #[test]
    fn anonymous_is_not_verified_and_keys_are_stable() {
        let a =
            request_authority(&request("anon1.{\"idempotency_key\":\"one-operation\"}")).unwrap();
        let b =
            request_authority(&request("anon1.{\"idempotency_key\":\"one-operation\"}")).unwrap();
        assert!(!a.is_authenticated());
        assert_eq!(a.principal(), "anonymous");
        assert_eq!(a.tenant(), "local");
        assert_eq!(a.idempotency_key(), b.idempotency_key());
        assert_ne!(a.attempt_nonce(), b.attempt_nonce());
    }
    #[test]
    fn anonymous_metadata_bounds_and_empty_keys_are_rejected() {
        for key in [
            " ".to_owned(),
            "a".repeat(257),
            "x\n".to_owned(),
            "x\u{0085}".to_owned(),
        ] {
            let raw = format!("anon1.{}", serde_json::json!({"idempotency_key": key}));
            assert!(request_authority(&request(&raw)).is_err());
        }
        assert!(request_authority(&request(&format!("anon1.{}", " ".repeat(2049)))).is_err());
    }
    // spec: MEDULLA-IDENTITY-R003.2
    #[test]
    fn anonymous_metadata_at_the_bound_admits_and_past_it_refuses_typed() {
        let body = "{\"idempotency_key\":\"k\"}";
        let at_bound = format!(
            "anon1.{body}{}",
            " ".repeat(MAX_METADATA_BYTES - body.len())
        );
        assert!(request_authority(&request(&at_bound)).is_ok());
        let past = format!("{at_bound} ");
        assert_eq!(
            request_authority(&request(&past)).unwrap_err(),
            OVERSIZE_REFUSAL
        );
        let long_key = format!(
            "anon1.{}",
            serde_json::json!({"idempotency_key": "a".repeat(MAX_CORRELATION_KEY_BYTES + 1)})
        );
        assert_eq!(
            request_authority(&request(&long_key)).unwrap_err(),
            OVERSIZE_REFUSAL
        );
    }
    // spec: MEDULLA-IDENTITY-R003.1
    #[test]
    fn anonymous_refuses_credentials_claims_and_invalid_metadata() {
        for value in [
            "eg2.credentials",
            "anon1.bad",
            "anon1.{}",
            "anon1.{\"idempotency_key\":\"\"}",
            "anon1.{\"idempotency_key\":\"x\",\"principal\":\"admin\"}",
            "anon1.{\"idempotency_key\":\"x\",\"mac\":\"signature\"}",
            "anon1.{\"idempotency_key\":\"x\",\"idempotency_key\":\"y\"}",
        ] {
            let refusal = request_authority(&request(value)).unwrap_err();
            assert!(refusal.starts_with(REFUSAL_PREFIX), "{value}: {refusal}");
        }
        let mut identity = request("");
        identity.agent_id = Some("admin".into());
        let refusal = request_authority(&identity).unwrap_err();
        assert!(refusal.starts_with(REFUSAL_PREFIX), "{refusal}");
    }
    // spec: MEDULLA-IDENTITY-R003.1
    #[test]
    fn anonymous_origin_pins_typed_local_authority_without_credentials() {
        // No secret and no signed envelope are required: the server-owned
        // origin fixes the local authority constants.
        let authority = request_authority(&request("")).unwrap();
        assert!(!authority.is_authenticated());
        assert_eq!(authority.principal(), "anonymous");
        assert_eq!(authority.tenant(), "local");
        assert_eq!(authority.claims().policy_version, "no-auth");
        assert!(!authority.idempotency_key().is_empty());
        // A signed envelope is a credential and caller identity is a claim:
        // both are refused, not substituted.
        for token in [
            "eg2.signed-envelope",
            "anon1.{\"principal\":\"anonymous\",\"tenant\":\"local\"}",
        ] {
            let refusal = request_authority(&request(token)).unwrap_err();
            assert!(refusal.starts_with(REFUSAL_PREFIX), "{token}: {refusal}");
        }
    }
}
