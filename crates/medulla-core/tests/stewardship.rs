//! Stewardship rules (MEDULLA-TYPED-PACKS-R802.2).

use medulla_core::stewardship::{
    certify, open_change_request, resolve, reviewers, transition, StewardshipError, TargetScopes,
    CERTIFICATION_TTL_DAYS_DEFAULT,
};
use medulla_types::stewardship::{
    ChangeRequestState, StewardRole, StewardshipAssignment, StewardshipScope,
};

fn assign(scope: StewardshipScope, role: StewardRole, who: &str) -> StewardshipAssignment {
    StewardshipAssignment {
        scope,
        role,
        principal: who.into(),
    }
}

fn scopes() -> TargetScopes {
    TargetScopes {
        asset: Some("table:orders".into()),
        concept_scheme: Some("scheme:revenue".into()),
        domain: Some("finance".into()),
    }
}

/// Verifies MEDULLA-TYPED-PACKS-R802.2: Behavior
// spec: MEDULLA-TYPED-PACKS-R802.2
#[test]
fn most_specific_assignment_wins_and_certification_expires() {
    let all = vec![
        assign(
            StewardshipScope::Domain("finance".into()),
            StewardRole::Owner,
            "dom",
        ),
        assign(
            StewardshipScope::ConceptScheme("scheme:revenue".into()),
            StewardRole::Steward,
            "scheme",
        ),
        assign(
            StewardshipScope::Domain("other".into()),
            StewardRole::Owner,
            "elsewhere",
        ),
    ];
    // No asset assignment: the concept scheme beats the domain.
    let found = resolve(&all, &scopes());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].principal, "scheme");

    // An asset assignment beats both; custodians never review.
    let mut with_asset = all.clone();
    with_asset.push(assign(
        StewardshipScope::Asset("table:orders".into()),
        StewardRole::Owner,
        "asset-b",
    ));
    with_asset.push(assign(
        StewardshipScope::Asset("table:orders".into()),
        StewardRole::Steward,
        "asset-a",
    ));
    with_asset.push(assign(
        StewardshipScope::Asset("table:orders".into()),
        StewardRole::Custodian,
        "ops",
    ));
    assert_eq!(
        reviewers(&with_asset, &scopes()),
        vec!["asset-a", "asset-b"]
    );

    // Nothing governs an unrelated target.
    let none = TargetScopes {
        asset: Some("table:x".into()),
        concept_scheme: None,
        domain: None,
    };
    assert!(resolve(&all, &none).is_empty());
    assert_eq!(
        open_change_request(
            "table:x",
            &format!("sha256:{}", "a".repeat(64)),
            &all,
            &none
        ),
        Err(StewardshipError::NoReviewers)
    );

    // Change request flow.
    let digest = format!("sha256:{}", "b".repeat(64));
    let request = open_change_request("table:orders", &digest, &with_asset, &scopes()).unwrap();
    assert_eq!(request.state, ChangeRequestState::Open);
    assert_eq!(request.reviewers, vec!["asset-a", "asset-b"]);
    use ChangeRequestState::{Applied, Approved, Open, Rejected};
    assert_eq!(transition(Open, Approved), Ok(Approved));
    assert_eq!(transition(Open, Rejected), Ok(Rejected));
    assert_eq!(transition(Approved, Applied), Ok(Applied));
    for (from, to) in [
        (Open, Applied),
        (Rejected, Approved),
        (Applied, Open),
        (Approved, Rejected),
    ] {
        assert_eq!(
            transition(from, to),
            Err(StewardshipError::IllegalTransition { from, to })
        );
    }

    // Certification lapses after the ttl and the knob range is enforced.
    let cert = certify(
        "table:orders",
        "gold",
        "asset-a",
        1_000,
        CERTIFICATION_TTL_DAYS_DEFAULT,
    )
    .unwrap();
    let ttl = 365 * 24 * 60 * 60;
    assert_eq!(cert.expires_at, 1_000 + ttl);
    assert!(cert.holds_at(1_000 + ttl - 1));
    assert!(!cert.holds_at(1_000 + ttl));
    assert_eq!(
        certify("a", "gold", "b", 1, 29),
        Err(StewardshipError::TtlOutOfRange(29))
    );
    assert_eq!(
        certify("a", "gold", "b", 1, 1096),
        Err(StewardshipError::TtlOutOfRange(1096))
    );
}
