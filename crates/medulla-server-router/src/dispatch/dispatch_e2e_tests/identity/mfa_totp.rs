//! The engine-held TOTP ops served end to end (MEDULLA-IDENTITY-R741): the engine generates and
//! seals the secret, answers the provisioning URI once, checks codes on its own clock, refuses a
//! replayed code, and locks the principal durably after repeated failures. No test sleeps.

use super::*;

fn tenant() -> String {
    medulla_server_identity::dispatch::identity_store::resolution_context::record_tenant(
        &medulla_server_api::server_context::ServerContext::fixture(),
        &broker(),
    )
    .expect("configured test policy binds the session tenant")
}

fn code_request(tenant: &str, code: &str) -> MfaCodeRequest {
    MfaCodeRequest {
        tenant: tenant.to_string(),
        session: Secret::new(SESSION),
        code: Secret::new(code),
    }
}

fn enroll() -> IdentityOp {
    IdentityOp::Mfa(MfaOp::Enroll {
        request: MfaEnrollRequest {
            tenant: tenant(),
            session: Secret::new(SESSION),
        },
    })
}

fn confirm(code: &str) -> IdentityOp {
    IdentityOp::Mfa(MfaOp::Confirm {
        request: code_request(&tenant(), code),
    })
}

fn disable(code: &str) -> IdentityOp {
    IdentityOp::Mfa(MfaOp::Disable {
        request: code_request(&tenant(), code),
    })
}

fn provisioning_uri(response: &Response) -> String {
    let reply: IdentityReply = match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).expect("reply"),
        other => panic!("expected a JSON identity reply, got {other:?}"),
    };
    match reply {
        IdentityReply::MfaProvisioning(provisioning) => provisioning.provisioning_uri,
        other => panic!("expected a provisioning reply, got {other:?}"),
    }
}

/// The engine's current code for the secret in `uri`, on the engine clock.
fn code_for(uri: &str) -> String {
    let secret = uri
        .split("secret=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .expect("the URI carries the secret");
    let key = secrets::base32_decode(secret).expect("base32 secret");
    secrets::totp_code_for_test(
        &key,
        medulla_kernel::kernel::dispatch_support::authoritative_now_ms() / 1_000,
    )
}

/// A six-digit code that matches no step around now.
fn wrong_code(valid: &str) -> String {
    let shifted = (valid.parse::<u32>().unwrap() + 500_000) % 1_000_000;
    format!("{shifted:06}")
}

// spec: MEDULLA-IDENTITY-R741
#[tokio::test]
async fn the_engine_holds_the_secret_refuses_replays_and_locks_after_failures() {
    let state = state();
    for op in [initialize(PASSWORD), sign_in(PASSWORD, SESSION)] {
        let response = send(&state, broker(), op).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    let enrolled = send(&state, broker(), enroll()).await;
    assert!(enrolled.error.is_none(), "{:?}", enrolled.error);
    let uri = provisioning_uri(&enrolled);
    assert!(
        uri.starts_with("otpauth://totp/medulla:root?secret="),
        "{uri}"
    );
    let valid = code_for(&uri);

    let foreign = send(
        &state,
        broker(),
        IdentityOp::Mfa(MfaOp::Confirm {
            request: code_request("tenant-foreign", &valid),
        }),
    )
    .await;
    assert_eq!(foreign.error.as_deref(), Some("IDENTITY_NOT_AUTHORIZED"));
    let invalid = send(&state, broker(), confirm(&wrong_code(&valid))).await;
    assert_eq!(invalid.error.as_deref(), Some("MFA_CODE_INVALID"));
    let confirmed = send(&state, broker(), confirm(&valid)).await;
    assert!(confirmed.error.is_none(), "{:?}", confirmed.error);

    let replayed = send(&state, broker(), disable(&valid)).await;
    assert_eq!(replayed.error.as_deref(), Some("MFA_CODE_REPLAYED"));

    for _ in 0..5 {
        let wrong = send(&state, broker(), disable(&wrong_code(&valid))).await;
        assert_eq!(wrong.error.as_deref(), Some("MFA_CODE_INVALID"));
    }
    let before = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let locked = send(&state, broker(), disable(&wrong_code(&valid))).await;
    let error = locked.error.expect("the sixth failure is locked");
    let retry_at: u64 = error
        .strip_prefix("MFA_LOCKED: retry_at_ms=")
        .unwrap_or_else(|| panic!("expected a lock with its retry time: {error}"))
        .parse()
        .unwrap();
    assert!(retry_at > before, "{retry_at} <= {before}");
}
