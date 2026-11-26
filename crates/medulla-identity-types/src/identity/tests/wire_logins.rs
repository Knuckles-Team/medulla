//! Wire login scopes and the reserved service agent id (MEDULLA-ENGINE-PARITY-R007.4).
use super::*;

fn grant(login: &str, principal: &str, scopes: Option<Vec<&str>>) -> IdentityOp {
    let request = WireLoginGrant {
        login: login.to_string(),
        kind: WireLoginKind::Token,
        verifier: "ab".repeat(32),
        principal: principal.to_string(),
        scopes: scopes.map_or_else(default_wire_login_scopes, |scopes| {
            scopes.into_iter().map(str::to_string).collect()
        }),
    };
    IdentityOp::WireLogin(WireLoginOp::Grant { request })
}

fn stamp() -> IdentityStamp {
    let mut stamp = admin();
    stamp.session_tenant = Some("tenant-one".into());
    stamp
}

fn granted(reply: IdentityReply) -> WireLoginGranted {
    match reply {
        IdentityReply::WireLoginGranted(granted) => granted,
        other => panic!("unexpected reply {other:?}"),
    }
}

// spec: MEDULLA-ENGINE-PARITY-R007.4
#[test]
fn a_grant_defaults_to_graph_scopes_and_an_admin_may_add_replication_read() {
    let mut store = store_in(AuthMode::Local);
    let plain = granted(
        apply_kept(&mut store, &grant("bi", "agent:bi", None), &stamp(), NOW).expect("default"),
    );
    assert_eq!(plain.scopes, ["kg:read", "kg:write"]);
    let replicator = apply_kept(
        &mut store,
        &grant(
            "feed",
            "agent:feed",
            Some(vec!["replication:read", "kg:read"]),
        ),
        &stamp(),
        NOW,
    )
    .expect("replication scope");
    assert_eq!(granted(replicator).scopes, ["kg:read", "replication:read"]);
    assert_eq!(
        store.wire_login("feed").unwrap().scopes,
        ["kg:read", "replication:read"]
    );
    // The same login with other scopes is a different grant.
    let again = apply_kept(
        &mut store,
        &grant("feed", "agent:feed", None),
        &stamp(),
        NOW,
    );
    assert_eq!(again.unwrap_err(), IdentityRefusal::WireLoginExists);
}

// spec: MEDULLA-ENGINE-PARITY-R007.4
#[test]
fn a_grant_refuses_unregistered_administrative_and_empty_scopes() {
    let mut store = store_in(AuthMode::Local);
    for (scopes, refusal) in [
        (vec!["nope:read"], IdentityRefusal::UnknownScope),
        (vec!["kg:admin"], IdentityRefusal::ClassViolation),
        (vec![], IdentityRefusal::InvalidRequest),
    ] {
        let result = apply_kept(
            &mut store,
            &grant("bad", "agent:bad", Some(scopes)),
            &stamp(),
            NOW,
        );
        assert_eq!(result.unwrap_err(), refusal);
    }
    assert!(store.wire_login("bad").is_none());
}

// spec: MEDULLA-DURABLE-KERNEL-R035.7.2
#[test]
fn the_replication_service_agent_id_is_reserved_at_issuance() {
    assert!(is_reserved_agent_id(REPLICATION_SLOT_AGENT));
    assert_eq!(
        validate_principal_id(REPLICATION_SLOT_AGENT),
        Err(IdentityRefusal::InvalidRequest)
    );
    assert!(validate_principal_id("service:other").is_ok());
    let mut store = store_in(AuthMode::Local);
    // A wire login cannot map to it.
    let login = apply_kept(
        &mut store,
        &grant("slots", REPLICATION_SLOT_AGENT, None),
        &stamp(),
        NOW,
    );
    assert_eq!(login.unwrap_err(), IdentityRefusal::InvalidRequest);
    // A user cannot be created under it.
    let mut create_stamp = admin();
    create_stamp.minted_principal_id = Some(REPLICATION_SLOT_AGENT.to_string());
    let op = IdentityOp::User(UserOp::Create {
        request: CreateUserRequest {
            username: "slots".to_string(),
            kind: UserKind::Service,
            principal_id: None,
            display_name: None,
            email: None,
            roles: BTreeSet::new(),
            groups: BTreeSet::new(),
            password: Secret::default(),
            must_change: false,
        },
    });
    assert_eq!(
        apply_kept(&mut store, &op, &create_stamp, NOW).unwrap_err(),
        IdentityRefusal::InvalidRequest
    );
}
