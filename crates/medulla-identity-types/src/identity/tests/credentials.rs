//! Spec pass 14 credential ops: administrator API keys, session revoke by handle, the issuer
//! key ring, SCIM client credentials, administrator reset tokens and the MFA challenge store
//! (MEDULLA-IDENTITY-R730 to R735). Every clock read is the `now_ms` of the apply context.

use super::*;

fn direct_admin(hashes: &[&str]) -> IdentityStamp {
    let mut stamp = admin();
    stamp.token_hashes = hashes.iter().map(|hash| hash.to_string()).collect();
    stamp
}

fn broker_with(hashes: &[&str]) -> IdentityStamp {
    let mut stamp = broker();
    stamp.token_hashes = hashes.iter().map(|hash| hash.to_string()).collect();
    stamp
}

fn issue_admin_key(principal: &str, key_id: &str, scopes: &[&str]) -> IdentityOp {
    IdentityOp::Token(TokenOp::IssueApiKeyAdmin {
        request: ApiKeyAdminIssue {
            principal_id: principal.to_string(),
            key_id: key_id.to_string(),
            secret: Secret::default(),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            ttl_ms: 60 * 60 * 1000,
        },
    })
}

// spec: MEDULLA-IDENTITY-R730
#[test]
fn an_administrator_issues_and_lists_api_keys_without_a_session_token() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();

    // No session exists: the direct administrator issues anyway.
    let op = issue_admin_key(&alice, "k-admin", &["kg:read"]);
    let stamp = direct_admin(&["k-admin-secret"]);
    apply_kept(&mut store, &op, &stamp, NOW).unwrap();

    // The broker is not an administrator, and a second issue under the same id collides.
    assert_eq!(
        apply_kept(&mut store, &op, &broker_with(&["other-secret"]), NOW),
        Err(IdentityRefusal::NotAuthorized)
    );
    assert_eq!(
        apply_kept(&mut store, &op, &stamp, NOW),
        Err(IdentityRefusal::Collision)
    );
    // A scope the owner does not hold is refused as it is for the session-backed issue.
    let wide = issue_admin_key(&alice, "k-wide", &["kg:write"]);
    assert_eq!(
        apply_kept(&mut store, &wide, &stamp, NOW),
        Err(IdentityRefusal::ClassViolation)
    );

    // The key verifies, and the listing shows it without any secret or hash.
    let verify = IdentityOp::Token(TokenOp::VerifyApiKey {
        request: ApiKeyUse {
            key_id: "k-admin".to_string(),
            secret: Secret::default(),
        },
    });
    assert!(apply_kept(&mut store, &verify, &broker_with(&["k-admin-secret"]), NOW).is_ok());
    let list = IdentityOp::Token(TokenOp::ListApiKeys {
        request: ObjectRef { id: alice.clone() },
    });
    let IdentityReply::ApiKeys(keys) = apply_kept(&mut store, &list, &admin(), NOW).unwrap() else {
        panic!("a listing answers keys");
    };
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key_id, "k-admin");
    assert_eq!(keys[0].principal_id, alice);
    assert!(!format!("{keys:?}").contains("secret"));

    // A principal with no keys, and one that does not exist, answer the same empty list.
    for principal in ["usr:nobody", BOOTSTRAP_PRINCIPAL] {
        let list = IdentityOp::Token(TokenOp::ListApiKeys {
            request: ObjectRef {
                id: principal.to_string(),
            },
        });
        assert_eq!(
            apply_kept(&mut store, &list, &admin(), NOW).unwrap(),
            IdentityReply::ApiKeys(Vec::new())
        );
    }
}

// spec: MEDULLA-IDENTITY-R731
#[test]
fn a_session_is_revoked_by_the_handle_a_listing_shows() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    open_session(&mut store, "alice", &alice, "alice-session");

    let list = IdentityOp::Session(SessionOp::List {
        request: ObjectRef { id: alice.clone() },
    });
    let IdentityReply::Sessions(sessions) = apply_kept(&mut store, &list, &admin(), NOW).unwrap()
    else {
        panic!("a listing answers sessions");
    };
    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].revoked);
    let handle = sessions[0].handle.clone();

    let revoke = IdentityOp::Session(SessionOp::RevokeByHandle {
        request: SessionHandleRef {
            handle: handle.clone(),
        },
    });
    // Only an administrator revokes by handle.
    assert_eq!(
        apply_kept(&mut store, &revoke, &broker(), NOW),
        Err(IdentityRefusal::NotAuthorized)
    );
    assert_eq!(
        apply_kept(&mut store, &revoke, &admin(), NOW).unwrap(),
        IdentityReply::Done { changed: true }
    );
    let IdentityReply::Sessions(after) = apply_kept(&mut store, &list, &admin(), NOW).unwrap()
    else {
        panic!("a listing answers sessions");
    };
    assert!(after[0].revoked);
    // Again, and a handle that names nothing, change nothing.
    assert_eq!(
        apply_kept(&mut store, &revoke, &admin(), NOW).unwrap(),
        IdentityReply::Done { changed: false }
    );
    let unknown = IdentityOp::Session(SessionOp::RevokeByHandle {
        request: SessionHandleRef {
            handle: "0".repeat(SESSION_HANDLE_CHARS),
        },
    });
    assert_eq!(
        apply_kept(&mut store, &unknown, &admin(), NOW).unwrap(),
        IdentityReply::Done { changed: false }
    );
    // A malformed handle is refused.
    let malformed = IdentityOp::Session(SessionOp::RevokeByHandle {
        request: SessionHandleRef {
            handle: "NOT-A-HANDLE".to_string(),
        },
    });
    assert_eq!(
        apply_kept(&mut store, &malformed, &admin(), NOW),
        Err(IdentityRefusal::InvalidRequest)
    );
}

fn rotate(kid: &str, algorithm: &str) -> IdentityOp {
    IdentityOp::Token(TokenOp::RotateIssuerKey {
        request: IssuerKeyRotate {
            kid: kid.to_string(),
            algorithm: algorithm.to_string(),
            public_key_hex: "ab".repeat(32),
        },
    })
}

fn ring(store: &mut IdentityStore, now_ms: u64) -> Vec<IssuerKeyView> {
    let read = IdentityOp::Token(TokenOp::ReadIssuerKeys);
    let IdentityReply::IssuerKeys(keys) = apply_kept(store, &read, &admin(), now_ms).unwrap()
    else {
        panic!("a read answers keys");
    };
    keys
}

// spec: MEDULLA-IDENTITY-R732
#[test]
fn the_issuer_key_ring_rotates_and_publishes_retired_keys_for_the_grace_only() {
    let mut store = store_in(AuthMode::Local);
    assert!(ring(&mut store, NOW).is_empty());

    apply_kept(&mut store, &rotate("key-1", "EdDSA"), &admin(), NOW).unwrap();
    let later = NOW + 1_000;
    let IdentityReply::IssuerKeys(rotated) =
        apply_kept(&mut store, &rotate("key-2", "EdDSA"), &admin(), later).unwrap()
    else {
        panic!("a rotation answers the ring");
    };
    assert_eq!(rotated.len(), 2);
    assert_eq!(
        (rotated[0].kid.as_str(), rotated[0].state.as_str()),
        ("key-2", "active")
    );
    assert_eq!(
        (rotated[1].kid.as_str(), rotated[1].state.as_str()),
        ("key-1", "retired")
    );

    // The retired key leaves the ring once its grace has passed; the active one stays.
    let after_grace = later + ISSUER_KEY_GRACE_MS + 1;
    let published = ring(&mut store, after_grace);
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].kid, "key-2");

    // Refusals: a known kid, an unknown algorithm, a malformed key, and the broker.
    assert_eq!(
        apply_kept(&mut store, &rotate("key-2", "EdDSA"), &admin(), later),
        Err(IdentityRefusal::Collision)
    );
    assert_eq!(
        apply_kept(&mut store, &rotate("key-3", "HS256"), &admin(), later),
        Err(IdentityRefusal::InvalidRequest)
    );
    let short = IdentityOp::Token(TokenOp::RotateIssuerKey {
        request: IssuerKeyRotate {
            kid: "key-4".to_string(),
            algorithm: "EdDSA".to_string(),
            public_key_hex: "zz".to_string(),
        },
    });
    assert_eq!(
        apply_kept(&mut store, &short, &admin(), later),
        Err(IdentityRefusal::InvalidRequest)
    );
    assert_eq!(
        apply_kept(&mut store, &rotate("key-5", "EdDSA"), &broker(), later),
        Err(IdentityRefusal::NotAuthorized)
    );
}

fn scim_create(client_id: &str) -> IdentityOp {
    IdentityOp::Token(TokenOp::CreateScimClient {
        request: ScimClientCreate {
            client_id: client_id.to_string(),
            label: "directory sync".to_string(),
            secret: Secret::default(),
            ttl_ms: 60 * 60 * 1000,
        },
    })
}

// spec: MEDULLA-IDENTITY-R733
#[test]
fn a_scim_client_credential_is_created_read_updated_and_deleted() {
    let mut store = store_in(AuthMode::Local);
    let created = apply_kept(
        &mut store,
        &scim_create("scim-1"),
        &direct_admin(&["scim-1-secret"]),
        NOW,
    )
    .unwrap();
    let IdentityReply::ScimClient(view) = created else {
        panic!("a create answers the client");
    };
    assert_eq!(view.client_id, "scim-1");
    assert_eq!(view.expires_at_ms, NOW + 60 * 60 * 1000);
    assert!(!format!("{view:?}").contains("secret"));

    // A second create collides; the broker may not create.
    assert_eq!(
        apply_kept(
            &mut store,
            &scim_create("scim-1"),
            &direct_admin(&["other-secret"]),
            NOW
        ),
        Err(IdentityRefusal::Collision)
    );
    assert_eq!(
        apply_kept(
            &mut store,
            &scim_create("scim-2"),
            &broker_with(&["s"]),
            NOW
        ),
        Err(IdentityRefusal::NotAuthorized)
    );

    let list = IdentityOp::Token(TokenOp::ListScimClients);
    let IdentityReply::ScimClients(listed) = apply_kept(&mut store, &list, &admin(), NOW).unwrap()
    else {
        panic!("a listing answers clients");
    };
    assert_eq!(listed.len(), 1);

    // An update changes the label and the lifetime, and rotates the secret hash.
    let update = IdentityOp::Token(TokenOp::UpdateScimClient {
        request: ScimClientUpdate {
            client_id: "scim-1".to_string(),
            label: Some("renamed".to_string()),
            secret: Some(Secret::default()),
            ttl_ms: Some(2 * 60 * 60 * 1000),
        },
    });
    let before = store.scim_clients["scim-1"].secret_hash.clone();
    let IdentityReply::ScimClient(updated) = apply_kept(
        &mut store,
        &update,
        &direct_admin(&["scim-1-secret-2"]),
        NOW + 5,
    )
    .unwrap() else {
        panic!("an update answers the client");
    };
    assert_eq!(updated.label, "renamed");
    assert_eq!(updated.expires_at_ms, NOW + 5 + 2 * 60 * 60 * 1000);
    assert_ne!(store.scim_clients["scim-1"].secret_hash, before);
    let empty = IdentityOp::Token(TokenOp::UpdateScimClient {
        request: ScimClientUpdate {
            client_id: "scim-1".to_string(),
            label: None,
            secret: None,
            ttl_ms: None,
        },
    });
    assert_eq!(
        apply_kept(&mut store, &empty, &admin(), NOW),
        Err(IdentityRefusal::InvalidRequest)
    );

    // Delete removes it; reading or deleting it again is not found.
    let get = IdentityOp::Token(TokenOp::GetScimClient {
        request: ObjectRef {
            id: "scim-1".to_string(),
        },
    });
    assert!(apply_kept(&mut store, &get, &admin(), NOW).is_ok());
    let delete = IdentityOp::Token(TokenOp::DeleteScimClient {
        request: ObjectRef {
            id: "scim-1".to_string(),
        },
    });
    assert!(apply_kept(&mut store, &delete, &admin(), NOW).is_ok());
    assert_eq!(
        apply_kept(&mut store, &get, &admin(), NOW),
        Err(IdentityRefusal::NotFound)
    );
    assert_eq!(
        apply_kept(&mut store, &delete, &admin(), NOW),
        Err(IdentityRefusal::NotFound)
    );
}

fn scim_verify(client_id: &str) -> IdentityOp {
    IdentityOp::Token(TokenOp::VerifyScimClient {
        request: ScimClientUse {
            client_id: client_id.to_string(),
            secret: Secret::default(),
        },
    })
}

// spec: MEDULLA-IDENTITY-R733
#[test]
fn a_scim_client_secret_verifies_and_every_failure_answers_alike() {
    let mut store = store_in(AuthMode::Local);
    apply_kept(
        &mut store,
        &scim_create("scim-1"),
        &direct_admin(&["scim-1-secret"]),
        NOW,
    )
    .unwrap();

    // The right secret verifies and answers the client without any secret.
    let IdentityReply::ScimClient(view) = apply_kept(
        &mut store,
        &scim_verify("scim-1"),
        &broker_with(&["scim-1-secret"]),
        NOW + 1,
    )
    .unwrap() else {
        panic!("a verify answers the client");
    };
    assert_eq!(view.client_id, "scim-1");
    assert!(!format!("{view:?}").contains("secret"));

    // A wrong secret, an unknown client and an expired client are one refusal.
    let wrong = apply_kept(
        &mut store,
        &scim_verify("scim-1"),
        &broker_with(&["other-secret"]),
        NOW + 1,
    );
    let unknown = apply_kept(
        &mut store,
        &scim_verify("scim-nope"),
        &broker_with(&["scim-1-secret"]),
        NOW + 1,
    );
    let expired = apply_kept(
        &mut store,
        &scim_verify("scim-1"),
        &broker_with(&["scim-1-secret"]),
        NOW + 60 * 60 * 1000,
    );
    assert_eq!(wrong, Err(IdentityRefusal::NotFound));
    assert_eq!(unknown, wrong);
    assert_eq!(expired, wrong);

    // A rotated secret replaces the old one at once.
    let rotate = IdentityOp::Token(TokenOp::UpdateScimClient {
        request: ScimClientUpdate {
            client_id: "scim-1".to_string(),
            label: None,
            secret: Some(Secret::default()),
            ttl_ms: None,
        },
    });
    apply_kept(
        &mut store,
        &rotate,
        &direct_admin(&["scim-1-secret-2"]),
        NOW + 2,
    )
    .unwrap();
    assert_eq!(
        apply_kept(
            &mut store,
            &scim_verify("scim-1"),
            &broker_with(&["scim-1-secret"]),
            NOW + 3
        ),
        Err(IdentityRefusal::NotFound)
    );
    assert!(apply_kept(
        &mut store,
        &scim_verify("scim-1"),
        &broker_with(&["scim-1-secret-2"]),
        NOW + 3
    )
    .is_ok());
}

// spec: MEDULLA-IDENTITY-R734
#[test]
fn an_administrator_reset_token_is_issued_directly_and_spent_once() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    let issue = IdentityOp::Token(TokenOp::IssueAdminReset {
        request: AdminResetIssue {
            principal_id: alice.clone(),
            token: Secret::default(),
            ttl_ms: 30 * 60 * 1000,
        },
    });
    // Only a direct administrator issues; an unknown principal is not found.
    assert_eq!(
        apply_kept(&mut store, &issue, &broker_with(&["reset-1"]), NOW),
        Err(IdentityRefusal::NotAuthorized)
    );
    let unknown = IdentityOp::Token(TokenOp::IssueAdminReset {
        request: AdminResetIssue {
            principal_id: "usr:nobody".to_string(),
            token: Secret::default(),
            ttl_ms: 30 * 60 * 1000,
        },
    });
    assert_eq!(
        apply_kept(&mut store, &unknown, &direct_admin(&["reset-x"]), NOW),
        Err(IdentityRefusal::NotFound)
    );
    let reply = apply_kept(&mut store, &issue, &direct_admin(&["reset-1"]), NOW).unwrap();
    assert!(matches!(reply, IdentityReply::ResetDelivery(_)));

    let redeem = |store: &IdentityStore| {
        let mut stamp = broker_with(&["reset-1"]);
        stamp.password_hash = Some("$argon2id$reset".to_string());
        stamp.password_check = Some(check_for(store, store.one_time_principal("reset-1"), false));
        let op = IdentityOp::Token(TokenOp::RedeemOneTime {
            request: TokenRedeem {
                purpose: TokenPurpose::AdminReset,
                token: Secret::default(),
                new_password: Secret::default(),
                link: None,
            },
        });
        (op, stamp)
    };
    let (op, stamp) = redeem(&store);
    assert!(apply_kept(&mut store, &op, &stamp, NOW + 1).is_ok());
    assert_eq!(
        apply_kept(&mut store, &op, &stamp, NOW + 2),
        Err(IdentityRefusal::TokenSpent),
        "a spent reset token is refused"
    );

    // A newer token replaces the outstanding one, and an expired one is spent.
    apply_kept(&mut store, &issue, &direct_admin(&["reset-2"]), NOW).unwrap();
    apply_kept(&mut store, &issue, &direct_admin(&["reset-3"]), NOW).unwrap();
    assert!(store.one_time_principal("reset-2").is_none());
    assert!(store.one_time_principal("reset-3").is_some());
}

fn challenge_issue(kind: MfaChallengeKind, challenge: &str) -> IdentityOp {
    IdentityOp::Mfa(MfaOp::IssueChallenge {
        request: MfaChallengeIssue {
            session_token: Secret::default(),
            kind,
            challenge: challenge.to_string(),
        },
    })
}

fn challenge_consume(kind: MfaChallengeKind) -> IdentityOp {
    IdentityOp::Mfa(MfaOp::ConsumeChallenge {
        request: MfaChallengeConsume {
            session_token: Secret::default(),
            kind,
        },
    })
}

// spec: MEDULLA-IDENTITY-R735
#[test]
fn an_mfa_challenge_is_single_use_and_expires_after_five_minutes_on_the_injected_clock() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    open_session(&mut store, "alice", &alice, "alice-session");
    let session = broker_with(&["alice-session"]);

    // A challenge is stored, then spent exactly once.
    let issue = challenge_issue(MfaChallengeKind::Webauthn, "Y2hhbGxlbmdl");
    apply_kept(&mut store, &issue, &session, NOW).unwrap();
    let consume = challenge_consume(MfaChallengeKind::Webauthn);
    let IdentityReply::MfaChallenge(spent) =
        apply_kept(&mut store, &consume, &session, NOW + 1).unwrap()
    else {
        panic!("a consume answers the challenge");
    };
    assert_eq!(spent.challenge, "Y2hhbGxlbmdl");
    assert_eq!(spent.expires_at_ms, NOW + MFA_CHALLENGE_TTL_MS);
    assert_eq!(
        apply_kept(&mut store, &consume, &session, NOW + 2),
        Err(IdentityRefusal::NotFound),
        "a spent challenge is gone"
    );

    // The challenge of the other kind is separate, and a newer one replaces the older.
    apply_kept(
        &mut store,
        &challenge_issue(MfaChallengeKind::Enrollment, "one"),
        &session,
        NOW,
    )
    .unwrap();
    apply_kept(
        &mut store,
        &challenge_issue(MfaChallengeKind::Enrollment, "two"),
        &session,
        NOW,
    )
    .unwrap();
    let IdentityReply::MfaChallenge(latest) = apply_kept(
        &mut store,
        &challenge_consume(MfaChallengeKind::Enrollment),
        &session,
        NOW + 3,
    )
    .unwrap() else {
        panic!("a consume answers the challenge");
    };
    assert_eq!(latest.challenge, "two");

    // The clock decides expiry: one millisecond before is spent fine, at the TTL it is gone.
    apply_kept(&mut store, &issue, &session, NOW).unwrap();
    assert_eq!(
        apply_kept(&mut store, &consume, &session, NOW + MFA_CHALLENGE_TTL_MS),
        Err(IdentityRefusal::NotFound)
    );
    apply_kept(&mut store, &issue, &session, NOW).unwrap();
    assert!(apply_kept(
        &mut store,
        &consume,
        &session,
        NOW + MFA_CHALLENGE_TTL_MS - 1
    )
    .is_ok());

    // A session that is not live holds no challenge, and the store survives a round trip.
    apply_kept(&mut store, &issue, &session, NOW).unwrap();
    let image = serde_json::to_string(&store).unwrap();
    let mut restored: IdentityStore = serde_json::from_str(&image).unwrap();
    assert!(apply_kept(&mut restored, &consume, &session, NOW + 10).is_ok());
    assert_eq!(
        apply_kept(
            &mut store,
            &consume,
            &broker_with(&["no-such-session"]),
            NOW
        ),
        Err(IdentityRefusal::NotAuthorized)
    );
}
