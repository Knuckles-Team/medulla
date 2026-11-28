//! The engine-held TOTP ops: enroll, confirm, verify and disable (MEDULLA-IDENTITY-R741).
//! Every clock read is the `now_ms` the test passes in; no test sleeps.

use super::auth::{
    apply_verdict, mfa_code_request, mfa_confirm, mfa_disable, mfa_enroll, mfa_stamp, outcome,
    sign_in, with_password, MFA_TENANT,
};
use super::*;

fn verify_op() -> IdentityOp {
    IdentityOp::Mfa(MfaOp::Verify {
        request: mfa_code_request(),
    })
}

/// Stamp `op`'s session with the secret the engine sealed and the matched step (`None`: wrong code).
fn stamped(session: &str, step: Option<u64>) -> IdentityStamp {
    let mut stamp = mfa_stamp(session);
    stamp.sealed_secret = Some("sealed".to_string());
    stamp.totp_step = step;
    stamp
}

/// Alice enrolled and confirmed, then signed in again: `mfa` owes its second factor.
fn alice_with_pending_session(store: &mut IdentityStore) -> String {
    let alice = with_password(store, "alice");
    open_session(store, "alice", &alice, "enrol");
    apply_kept(store, &mfa_enroll(), &stamped("enrol", None), NOW).unwrap();
    let confirmed = apply_kept(store, &mfa_confirm(), &stamped("enrol", Some(100)), NOW).unwrap();
    assert_eq!(outcome(confirmed).outcome, AuthenticateOutcome::Ok);
    let again = apply_verdict(store, &sign_in("alice"), (Some(&alice), true, "mfa"), NOW);
    assert_eq!(
        outcome(again.unwrap()).outcome,
        AuthenticateOutcome::MfaRequired
    );
    alice
}

// spec: MEDULLA-IDENTITY-R741
#[test]
fn enroll_confirm_verify_and_disable_complete_one_lifecycle() {
    let mut store = store_in(AuthMode::Local);
    let alice = alice_with_pending_session(&mut store);
    assert!(store.mfa_enrolled(&alice));
    let verified = apply_kept(&mut store, &verify_op(), &stamped("mfa", Some(101)), NOW).unwrap();
    assert_eq!(outcome(verified).outcome, AuthenticateOutcome::Ok);
    // A fresh code on the completed session removes the factor.
    let disabled = apply_kept(&mut store, &mfa_disable(), &stamped("mfa", Some(102)), NOW).unwrap();
    assert_eq!(outcome(disabled).outcome, AuthenticateOutcome::Ok);
    assert!(!store.mfa_enrolled(&alice));
    assert_eq!(
        apply_kept(&mut store, &mfa_disable(), &stamped("mfa", Some(103)), NOW),
        Err(IdentityRefusal::MfaEnrollmentRequired),
        "a principal with no factor has nothing to disable"
    );
}

// spec: MEDULLA-IDENTITY-R741
#[test]
fn a_replayed_code_is_refused_and_changes_nothing() {
    let mut store = store_in(AuthMode::Local);
    alice_with_pending_session(&mut store);
    let before = serde_json::to_value(&store).unwrap();
    assert_eq!(
        store.apply(&verify_op(), &stamped("mfa", Some(100)), &ctx_at(NOW)),
        Err(IdentityRefusal::MfaCodeReplayed),
        "the confirmation step cannot step the session up"
    );
    assert_eq!(serde_json::to_value(&store).unwrap(), before);
    apply_kept(&mut store, &verify_op(), &stamped("mfa", Some(101)), NOW).unwrap();
    let before = serde_json::to_value(&store).unwrap();
    assert_eq!(
        store.apply(&mfa_disable(), &stamped("mfa", Some(101)), &ctx_at(NOW)),
        Err(IdentityRefusal::MfaCodeReplayed),
        "disable needs a code fresher than the one that stepped the session up"
    );
    assert_eq!(serde_json::to_value(&store).unwrap(), before);
}

// spec: MEDULLA-IDENTITY-R741
#[test]
fn failed_codes_lock_the_principal_durably_until_retry_at_on_the_injected_clock() {
    let mut store = store_in(AuthMode::Local);
    let alice = alice_with_pending_session(&mut store);
    let wrong = stamped("mfa", None);
    for _ in 0..5 {
        let result = apply_kept(&mut store, &verify_op(), &wrong, NOW).unwrap();
        assert_eq!(outcome(result).outcome, AuthenticateOutcome::Bad);
    }
    // The failures survive a serialize round trip: the limit is durable, not in memory.
    let mut store: IdentityStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    let locked =
        outcome(apply_kept(&mut store, &verify_op(), &stamped("mfa", Some(101)), NOW).unwrap());
    assert_eq!(locked.outcome, AuthenticateOutcome::Throttled);
    let retry_after_ms = locked
        .retry_after_ms
        .expect("a locked answer names the wait");
    assert!(retry_after_ms > 0);
    assert!(
        store.mfa_enrolled(&alice),
        "a locked attempt consumed nothing"
    );
    let open = apply_kept(
        &mut store,
        &verify_op(),
        &stamped("mfa", Some(101)),
        NOW + retry_after_ms,
    )
    .unwrap();
    assert_eq!(outcome(open).outcome, AuthenticateOutcome::Ok);
}

// spec: MEDULLA-IDENTITY-R741
#[test]
fn a_foreign_tenant_or_unknown_session_answers_the_same_refusal() {
    let mut store = store_in(AuthMode::Local);
    alice_with_pending_session(&mut store);
    let foreign = IdentityOp::Mfa(MfaOp::Verify {
        request: MfaCodeRequest {
            tenant: format!("not-{MFA_TENANT}"),
            ..mfa_code_request()
        },
    });
    let on_foreign = apply_kept(&mut store, &foreign, &stamped("mfa", Some(101)), NOW);
    let on_unknown = apply_kept(
        &mut store,
        &verify_op(),
        &stamped("no-such", Some(101)),
        NOW,
    );
    assert_eq!(on_foreign, Err(IdentityRefusal::NotAuthorized));
    assert_eq!(on_foreign, on_unknown);
}

// spec: MEDULLA-IDENTITY-R741
#[test]
fn confirm_and_verify_need_an_enrolment() {
    let mut store = store_in(AuthMode::Local);
    let alice = with_password(&mut store, "alice");
    open_session(&mut store, "alice", &alice, "plain");
    assert_eq!(
        apply_kept(&mut store, &mfa_confirm(), &stamped("plain", Some(5)), NOW),
        Err(IdentityRefusal::MfaEnrollmentRequired)
    );
    assert_eq!(
        apply_kept(&mut store, &mfa_disable(), &stamped("plain", Some(5)), NOW),
        Err(IdentityRefusal::MfaEnrollmentRequired)
    );
}
