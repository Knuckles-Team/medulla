//! Dashboard asset rules (MEDULLA-TYPED-PACKS-R109.2).

use medulla_core::dashboard::{
    access_of, delete, links_of, purge_due, restore, save, share, view, Access, Caller,
    DashboardRefusal, DashboardState,
};
use medulla_types::dashboard::{
    DashboardBody, DashboardGrant, DashboardPanel, DashboardRole, DASHBOARD_RETENTION_DAYS_DEFAULT,
};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn body(sources: &[&str], terms: &[&str]) -> DashboardBody {
    DashboardBody {
        panels: sources
            .iter()
            .enumerate()
            .map(|(n, source)| DashboardPanel {
                id: format!("p{n}"),
                source: (*source).to_string(),
                query: "SELECT 1".to_string(),
                terms: terms.iter().map(|t| (*t).to_string()).collect(),
                concepts: vec![],
            })
            .collect(),
        layout: serde_json::Value::Null,
        parameters: serde_json::Value::Null,
    }
}

fn state_of(plan: medulla_core::dashboard::SavePlan) -> DashboardState {
    DashboardState {
        asset: plan.asset,
        removed_at_ms: None,
        usage_count: 0,
        last_viewed_ms: None,
        purged: false,
    }
}

// spec: MEDULLA-TYPED-PACKS-R109.2
#[test]
fn save_bumps_revision_rebuilds_links_and_delete_keeps_then_purges() {
    let groups: Vec<String> = vec!["group:sales".into()];
    let owner = Caller { principal: "alice", groups: &[] };

    // A new dashboard saves at revision 1 and the saver owns it.
    let first = save(None, "d1", body(&["orders"], &["Order"]), 0, &owner, 10).unwrap();
    assert_eq!(first.asset.revision, 1);
    assert_eq!(first.asset.owner, "alice");
    assert_eq!(first.prior_revision, 0);
    assert_eq!(first.links.sources, vec!["orders".to_string()]);
    let mut state = state_of(first);

    // A stale revision is refused and names the current one.
    let stale = save(Some(&state), "d1", body(&["orders"], &[]), 0, &owner, 20).unwrap_err();
    assert_eq!(stale, DashboardRefusal::RevisionConflict { current: 1 });

    // The next save raises the revision by one and rebuilds the links from the new panels.
    let second = save(Some(&state), "d1", body(&["orders", "refunds"], &[]), 1, &owner, 30).unwrap();
    assert_eq!(second.asset.revision, 2);
    assert_eq!(second.prior_revision, 1);
    assert_eq!(links_of(&second.asset.body).sources, vec!["orders".to_string(), "refunds".to_string()]);
    assert!(second.links.terms.is_empty());
    state = state_of(second);

    // A grant names a group; a view grant reads and cannot save, and a stranger reads nothing.
    state.asset.grants = share(
        &state,
        vec![DashboardGrant { principal_or_group: "group:sales".into(), role: DashboardRole::View }],
        &owner,
    )
    .unwrap();
    let member = Caller { principal: "bob", groups: &groups };
    let stranger = Caller { principal: "eve", groups: &[] };
    assert_eq!(access_of(&state.asset, &member), Some(Access::View));
    assert_eq!(access_of(&state.asset, &stranger), None);
    assert_eq!(
        save(Some(&state), "d1", body(&["orders"], &[]), 2, &member, 40).unwrap_err(),
        DashboardRefusal::NotGranted
    );
    assert_eq!(view(&state, &stranger, 40).unwrap_err(), DashboardRefusal::NotGranted);
    assert_eq!(
        share(&state, vec![], &member).unwrap_err(),
        DashboardRefusal::NotGranted
    );
    let (usage, viewed) = view(&state, &member, 50).unwrap();
    assert_eq!((usage, viewed), (1, 50));

    // A panel with no query is refused.
    let mut empty = body(&["orders"], &[]);
    empty.panels[0].query = " ".into();
    assert!(matches!(
        save(Some(&state), "d1", empty, 2, &owner, 60).unwrap_err(),
        DashboardRefusal::BodyInvalid(_)
    ));

    // Delete keeps the body; a restore inside the retention brings it back.
    let removed = delete(&state, &owner, 100).unwrap();
    state.removed_at_ms = Some(removed);
    assert!(!purge_due(&state, 100 + DAY_MS, DASHBOARD_RETENTION_DAYS_DEFAULT));
    assert_eq!(view(&state, &member, 101).unwrap_err(), DashboardRefusal::NotGranted);
    restore(&state, &owner, 100 + DAY_MS, DASHBOARD_RETENTION_DAYS_DEFAULT).unwrap();

    // After the retention the purge is due and a restore is refused as purged.
    let after = 100 + DASHBOARD_RETENTION_DAYS_DEFAULT * DAY_MS;
    assert!(!purge_due(&state, after - 1, DASHBOARD_RETENTION_DAYS_DEFAULT));
    assert!(purge_due(&state, after, DASHBOARD_RETENTION_DAYS_DEFAULT));
    assert_eq!(
        restore(&state, &owner, after, DASHBOARD_RETENTION_DAYS_DEFAULT).unwrap_err(),
        DashboardRefusal::Purged
    );

    // A purged dashboard is no longer due and a read of it is refused as purged.
    state.purged = true;
    assert!(!purge_due(&state, after + DAY_MS, DASHBOARD_RETENTION_DAYS_DEFAULT));
    assert_eq!(view(&state, &owner, after + DAY_MS).unwrap_err(), DashboardRefusal::Purged);
}
