//! First run and the mode state machine.

use super::*;

fn transition(to: AuthMode, epoch: u64) -> IdentityOp {
    IdentityOp::Config(ConfigOp::Transition {
        request: ModeTransition {
            expected_epoch: epoch,
            to,
            local_fallback: None,
            issuer_kid: format!("kid-{epoch}"),
        },
    })
}

fn set_password(store: &IdentityStore, principal: &str) -> (IdentityOp, IdentityStamp) {
    super::auth::set_password_op(store, principal, "$argon2id$set")
}

#[test]
fn initialize_runs_once_and_only_for_the_first_run_authority() {
    let mut store = IdentityStore::default();
    let op = IdentityOp::Config(ConfigOp::Initialize {
        request: InitializeRequest {
            mode: AuthMode::None,
            admin_username: None,
            admin_password: Secret::default(),
        },
    });
    let reader = IdentityStamp::for_actor(actor("usr:x", &[IDENTITY_READ_SCOPE]));
    assert_eq!(
        store.apply(&op, &reader, &ctx_at(NOW)),
        Err(IdentityRefusal::NotAuthorized)
    );
    let reply = store.apply(&op, &broker(), &ctx_at(NOW)).unwrap();
    assert_eq!(
        reply,
        IdentityReply::Principal {
            principal_id: BOOTSTRAP_PRINCIPAL.to_string()
        }
    );
    assert_eq!(store.config().unwrap().mode, AuthMode::None);
    assert_eq!(
        store.config().unwrap().registration_policy,
        RegistrationPolicy::Disabled
    );
    assert_eq!(
        store.apply(&op, &broker(), &ctx_at(NOW)),
        Err(IdentityRefusal::AlreadyInitialized)
    );
}

#[test]
fn local_initialize_requires_the_first_admin_credential() {
    let mut store = IdentityStore::default();
    let op = crate::identity_fixtures::identity::bootstrap_local_op();
    assert_eq!(
        store.clone().apply(&op, &broker(), &ctx_at(NOW)),
        Err(IdentityRefusal::Unstamped)
    );
    let mut stamp = broker();
    stamp.password_hash = Some("$argon2id$first".to_string());
    store.apply(&op, &stamp, &ctx_at(NOW)).unwrap();
    assert_eq!(
        store.config().unwrap().registration_policy,
        RegistrationPolicy::AdminOnly
    );
    assert!(store.credential_of(BOOTSTRAP_PRINCIPAL).is_some());
}

#[test]
fn every_op_but_initialize_needs_an_initialized_store() {
    let mut store = IdentityStore::default();
    let op = IdentityOp::Config(ConfigOp::Get);
    assert_eq!(
        store.apply(&op, &broker(), &ctx_at(NOW)),
        Err(IdentityRefusal::NotInitialized)
    );
}

/// A store whose mode reads `external`, flipped from a `local` store the
/// same way the token tests build one: its administrator keeps a password.
fn external_store(store: IdentityStore) -> IdentityStore {
    let mut json = serde_json::to_value(&store).unwrap();
    json["config"]["mode"] = serde_json::json!("external");
    serde_json::from_value(json).unwrap()
}

/// `store` with the mode read as `none`.
fn none_store() -> IdentityStore {
    let mut json = serde_json::to_value(store_in(AuthMode::Local)).unwrap();
    json["config"]["mode"] = serde_json::json!("none");
    serde_json::from_value(json).unwrap()
}

// spec: MEDULLA-IDENTITY-R003.4
#[test]
fn transition_to_or_from_none_is_forbidden() {
    let edges = [
        (none_store(), AuthMode::Local),
        (none_store(), AuthMode::External),
        (store_in(AuthMode::Local), AuthMode::None),
        (external_store(store_in(AuthMode::Local)), AuthMode::None),
    ];
    for (mut store, to) in edges {
        let before = store.clone();
        // A stale epoch too: the `none` refusal comes before any other check.
        for epoch in [1, 9] {
            assert_eq!(
                apply_kept(&mut store, &transition(to, epoch), &admin(), NOW),
                Err(IdentityRefusal::ModeTransitionForbidden)
            );
        }
        assert_eq!(store, before, "a refusal changes nothing");
    }
    assert_eq!(
        IdentityRefusal::ModeTransitionForbidden.as_str(),
        "MODE_TRANSITION_FORBIDDEN"
    );
}

// spec: MEDULLA-IDENTITY-R003.4
#[test]
fn stale_epoch_refuses_and_changes_nothing() {
    let mut store = external_store(store_in(AuthMode::Local));
    let before = store.clone();
    assert_eq!(
        apply_kept(&mut store, &transition(AuthMode::Local, 7), &admin(), NOW),
        Err(IdentityRefusal::EpochConflict)
    );
    assert_eq!(store, before, "a stale epoch changes nothing");
    let IdentityReply::Config(config) =
        apply_kept(&mut store, &transition(AuthMode::Local, 1), &admin(), NOW).unwrap()
    else {
        panic!("transition answers the config");
    };
    assert_eq!(config.mode, AuthMode::Local);
    assert_eq!(config.epoch, 2);
    assert_eq!(config.issuer_kid_current.as_deref(), Some("kid-1"));
    assert_eq!(
        apply_kept(
            &mut store,
            &transition(AuthMode::External, 1),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::EpochConflict),
        "the epoch the accepted transition consumed is stale"
    );
}

// spec: MEDULLA-IDENTITY-R003.5
#[test]
fn unmet_precondition_refuses_typed() {
    let mut local = store_in(AuthMode::Local);
    let before = local.clone();
    assert_eq!(
        apply_kept(
            &mut local,
            &transition(AuthMode::External, 1),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::PreconditionFailed),
        "no enabled IdP links an administrator"
    );
    assert_eq!(
        apply_kept(&mut local, &transition(AuthMode::Local, 1), &admin(), NOW),
        Err(IdentityRefusal::IllegalTransition)
    );
    assert_eq!(local, before);

    let mut json = serde_json::to_value(external_store(store_in(AuthMode::Local))).unwrap();
    json["passwords"] = serde_json::json!({});
    let mut external: IdentityStore = serde_json::from_value(json).unwrap();
    let before = external.clone();
    assert_eq!(
        apply_kept(
            &mut external,
            &transition(AuthMode::Local, 1),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::PreconditionFailed),
        "an administrator without a password blocks external -> local"
    );
    assert_eq!(
        apply_kept(
            &mut external,
            &transition(AuthMode::External, 1),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::IllegalTransition)
    );
    assert_eq!(external, before, "a refusal changes nothing");
}

// spec: MEDULLA-IDENTITY-R003
#[test]
fn a_stale_epoch_and_an_illegal_edge_are_refused() {
    let mut store = store_in(AuthMode::Local);
    assert_eq!(
        apply_kept(
            &mut store,
            &transition(AuthMode::External, 7),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::EpochConflict)
    );
    assert_eq!(
        apply_kept(&mut store, &transition(AuthMode::Local, 1), &admin(), NOW),
        Err(IdentityRefusal::IllegalTransition)
    );
    assert_eq!(
        apply_kept(
            &mut store,
            &transition(AuthMode::External, 1),
            &admin(),
            NOW
        ),
        Err(IdentityRefusal::PreconditionFailed),
        "no enabled IdP links an administrator yet"
    );
}

#[test]
fn a_delegated_administrator_cannot_transition() {
    let mut store = store_in(AuthMode::Local);
    let mut stamp = admin();
    stamp.actor.delegated = true;
    let op = transition(AuthMode::External, 1);
    assert_eq!(
        apply_kept(&mut store, &op, &stamp, NOW),
        Err(IdentityRefusal::NotAuthorized)
    );
}

// spec: MEDULLA-IDENTITY-R003
#[test]
fn every_transition_revokes_every_session_and_keeps_every_account() {
    let mut store = store_in(AuthMode::Local);
    let mut sign_in = broker();
    sign_in.password_check = Some(check_for(&store, Some(BOOTSTRAP_PRINCIPAL), true));
    sign_in.token_hashes = vec!["session-hash".to_string()];
    let authenticate = IdentityOp::Credential(CredentialOp::Authenticate {
        request: AuthenticateRequest {
            username: "root-admin".to_string(),
            password: Secret::default(),
            session_token: Secret::default(),
            ip_prefix: None,
            new_password: Secret::default(),
        },
    });
    apply_kept(&mut store, &authenticate, &sign_in, NOW).unwrap();
    assert!(store.session_principal("session-hash", NOW).is_some());
    let mut store = external_store(store);
    let op = transition(AuthMode::Local, 1);
    apply_kept(&mut store, &op, &admin(), NOW).unwrap();
    assert!(store.session_principal("session-hash", NOW).is_none());
    assert!(store.manages(BOOTSTRAP_PRINCIPAL));
    assert!(store.credential_of(BOOTSTRAP_PRINCIPAL).is_some());
}

#[test]
fn bootstrap_sessions_exist_only_in_none_mode() {
    let mut none = store_in(AuthMode::None);
    let mut stamp = broker();
    stamp.token_hashes = vec!["demo-session".to_string()];
    let op = IdentityOp::Credential(CredentialOp::BootstrapSession {
        request: SessionTouch {
            refresh: None,
            session_token: Secret::default(),
            code: Secret::default(),
        },
    });
    apply_kept(&mut none, &op, &stamp, NOW).unwrap();
    assert_eq!(
        none.session_principal("demo-session", NOW),
        Some(BOOTSTRAP_PRINCIPAL)
    );
    let mut local = store_in(AuthMode::Local);
    assert_eq!(
        apply_kept(&mut local, &op, &stamp, NOW),
        Err(IdentityRefusal::PreconditionFailed)
    );
}

#[test]
fn update_policy_is_epoch_guarded_and_bounded() {
    let mut store = store_in(AuthMode::Local);
    let update = |epoch, min| {
        IdentityOp::Config(ConfigOp::UpdatePolicy {
            request: PolicyUpdate {
                expected_epoch: epoch,
                registration_policy: Some(RegistrationPolicy::Invite),
                local_fallback: None,
                password_min_chars: Some(min),
            },
        })
    };
    assert_eq!(
        apply_kept(&mut store, &update(1, 4), &admin(), NOW),
        Err(IdentityRefusal::InvalidRequest)
    );
    assert_eq!(
        apply_kept(&mut store, &update(9, 14), &admin(), NOW),
        Err(IdentityRefusal::EpochConflict)
    );
    apply_kept(&mut store, &update(1, 14), &admin(), NOW).unwrap();
    let config = store.config().unwrap();
    assert_eq!(config.password_min_chars, 14);
    assert_eq!(config.registration_policy, RegistrationPolicy::Invite);
}
