//! `Method::RevisionProposalList` served by its handler
//! (MEDULLA-DECISION-ENGINE-R817): the list is the verified tenant's own rows in
//! creation order, paged by cursor, never above the registry page ceiling, and
//! a malformed cursor refuses by name.

use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::{
    RevisionProposalListRequest, RevisionProposalPage, RevisionRow,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::super::stat_proposals::{handle_revision_proposal_list, record_revision};
use super::*;

const OTHER: &str = "tenant-other";

fn row(id: &str, created_ms: u64, stage: RevisionStage) -> RevisionRow {
    RevisionRow {
        id: id.to_string(),
        subject_kind: "program".to_string(),
        subject_ref: "p1".to_string(),
        stage,
        digest: format!("sha256:{id}"),
        evaluation_receipt_id: Some(format!("receipt:{id}")),
        created_ms,
    }
}

async fn list(
    h: &Harness,
    tenant: &str,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<RevisionProposalPage, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("reader", tenant);
    decode(
        handle_revision_proposal_list(
            &h.state,
            7,
            &verified,
            RevisionProposalListRequest { cursor, limit },
        )
        .await,
    )
}

/// Verifies MEDULLA-DECISION-ENGINE-R817: the list is tenant scoped and paged.
// spec: MEDULLA-DECISION-ENGINE-R817
#[tokio::test]
async fn list_is_tenant_scoped_and_paged() {
    let h = Harness::new().await;
    for (id, at) in [("c", 30), ("a", 10), ("b", 20)] {
        record_revision(&h.store, TENANT, &row(id, at, RevisionStage::Evaluated)).unwrap();
    }
    record_revision(&h.store, OTHER, &row("x", 15, RevisionStage::Draft)).unwrap();

    // Creation order, whatever the write order was.
    let all = list(&h, TENANT, None, None).await.unwrap();
    let ids: Vec<&str> = all.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(all.next_cursor, None);
    assert_eq!(
        all.rows[0].evaluation_receipt_id.as_deref(),
        Some("receipt:a")
    );

    // A page of two resumes after its cursor and ends without one.
    let first = list(&h, TENANT, None, Some(2)).await.unwrap();
    assert_eq!(first.rows.len(), 2);
    let cursor = first.next_cursor.clone().expect("more rows follow");
    let second = list(&h, TENANT, Some(cursor.clone()), Some(2))
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].id, "c");
    assert_eq!(second.next_cursor, None);

    // The other tenant sees only its own row, and the first tenant's cursor
    // resumes inside its own rows, never the first tenant's.
    let other = list(&h, OTHER, None, None).await.unwrap();
    assert_eq!(other.rows.len(), 1);
    assert_eq!(other.rows[0].id, "x");
    let other_resumed = list(&h, OTHER, Some(cursor), None).await.unwrap();
    assert!(other_resumed.rows.is_empty());

    // A tenant with no rows reads an empty page.
    assert!(list(&h, "tenant-none", None, None)
        .await
        .unwrap()
        .rows
        .is_empty());
}

/// Verifies MEDULLA-DECISION-ENGINE-R817: a malformed cursor refuses by name.
// spec: MEDULLA-DECISION-ENGINE-R817
#[tokio::test]
async fn malformed_cursor_refuses_by_name() {
    let h = Harness::new().await;
    for bad in ["", "x", "12:a", "00000000000000000005:"] {
        let refused = list(&h, TENANT, Some(bad.to_string()), None).await;
        assert!(
            refused.unwrap_err().starts_with("PROPOSAL_CURSOR_INVALID"),
            "{bad}"
        );
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R817: a new champion retires the subject's
/// previous champion row, and the list shows both with their stages.
// spec: MEDULLA-DECISION-ENGINE-R817
#[tokio::test]
async fn new_champion_retires_the_previous_one() {
    let h = Harness::new().await;
    record_revision(&h.store, TENANT, &row("old", 10, RevisionStage::Champion)).unwrap();
    record_revision(&h.store, TENANT, &row("new", 20, RevisionStage::Champion)).unwrap();
    let all = list(&h, TENANT, None, None).await.unwrap();
    let stages: Vec<(&str, RevisionStage)> =
        all.rows.iter().map(|r| (r.id.as_str(), r.stage)).collect();
    assert_eq!(
        stages,
        [
            ("old", RevisionStage::Retired),
            ("new", RevisionStage::Champion)
        ]
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R817: a request limit above the registry
/// ceiling is held to it, and a zero limit still returns one row.
// spec: MEDULLA-DECISION-ENGINE-R817
#[tokio::test]
async fn limit_is_held_to_the_registry_ceiling() {
    let h = Harness::new().await;
    let ceiling = medulla_server_api::knob_config::knobs().revision_proposals_page_limit;
    for at in 1..=(ceiling + 1) {
        record_revision(
            &h.store,
            TENANT,
            &row(&format!("r{at:05}"), at, RevisionStage::Draft),
        )
        .unwrap();
    }
    let page = list(&h, TENANT, None, Some(u32::MAX)).await.unwrap();
    assert_eq!(page.rows.len() as u64, ceiling);
    assert!(page.next_cursor.is_some());
    let one = list(&h, TENANT, None, Some(0)).await.unwrap();
    assert_eq!(one.rows.len(), 1);
}
