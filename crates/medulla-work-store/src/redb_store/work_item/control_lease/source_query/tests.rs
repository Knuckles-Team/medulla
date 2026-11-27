use super::*;
use crate::redb_store::work_item::test_shard::open;
use crate::redb_store::work_item::test_shard::with_node_rows;
use crate::redb_store::work_item::test_shard::GRAPH;
use medulla_result_types::source_query::{
    SourceQueryDialect, SourceQueryLimits, SourceQueryRequest,
};
use medulla_work_types::source_query_approval::SourceQueryAdmission;

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn request() -> ApproveSourceQueryRequest {
    ApproveSourceQueryRequest {
        tenant: "tenant-a".into(),
        query: SourceQueryRequest {
            schema_version: 1,
            source_ref: "src".into(),
            source_revision: "1".into(),
            schema_digest: digest('a'),
            snapshot_ref: digest('b'),
            dialect: SourceQueryDialect::Postgres,
            sql: "SELECT secret_column FROM t".into(),
            parameters: Vec::new(),
            limits: SourceQueryLimits {
                max_rows: 10,
                max_bytes: 1_048_576,
                chunk_rows: 5,
                chunk_bytes: 65_536,
                queue_depth: 2,
                queue_bytes: 1_048_576,
                deadline_ms: 10_000,
                max_resident_bytes: 8_388_608,
            },
            requested_expires_at_ms: 100_000,
            idempotency_key: "idem".into(),
        },
        admission: Some(SourceQueryAdmission {
            approval_id: "source-query-approval:one".into(),
            tenant_ref: "tenant-a".into(),
            owner_ref: "owner".into(),
            actor_ref: "actor".into(),
            authority_digest: digest('c'),
            policy_digest: digest('d'),
            identity_digest: digest('e'),
            approval_ttl_ms: 60_000,
        }),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R085.2
#[test]
fn native_issuer_writes_one_lease_whose_grant_holds_digests_only() {
    let temp = open("source-query-native");
    with_node_rows(&temp.shard, "source-query-issuer", |nodes, transitions| {
        // No admitted actor, or no admission, refuses and writes nothing.
        assert!(issue(
            GRAPH,
            &request(),
            None,
            nodes,
            transitions,
            DurableCrypto::none(),
            1_000
        )
        .is_err());
        let mut bare = request();
        bare.admission = None;
        let denied = issue(
            GRAPH,
            &bare,
            Some("native-actor"),
            nodes,
            transitions,
            DurableCrypto::none(),
            1_000,
        )
        .unwrap_err();
        assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
        assert!(load_row(
            nodes,
            GRAPH,
            "source-query-approval:one",
            DurableCrypto::none()
        )?
        .is_none());

        let issued = issue(
            GRAPH,
            &request(),
            Some("native-actor"),
            nodes,
            transitions,
            DurableCrypto::none(),
            1_000,
        )?;
        assert_eq!(issued.outcome, ControlLeaseIssueOutcome::Issued);
        let approved = issued.approved.expect("the approval travels in the reply");
        assert_eq!(
            approved.query_digest,
            approved.compute_query_digest().unwrap()
        );
        assert_eq!(approved.issued_at_ms, 1_000);
        assert_eq!(approved.hard_expires_at_ms, 61_000);
        assert_eq!(approved.expires_at_ms, 61_000);

        let row = load_row(
            nodes,
            GRAPH,
            "source-query-approval:one",
            DurableCrypto::none(),
        )?
        .expect("one lease row");
        assert_eq!(row["kind"], "source.query.approval");
        let stored = serde_json::to_string(&row).expect("the row serialises");
        assert!(
            !stored.contains("secret_column"),
            "the grant holds no SQL text"
        );
        assert_eq!(
            row["grant"]["source_query_approval"]["query_digest"],
            approved.query_digest
        );

        // The same approval id collides and writes nothing.
        let collision = issue(
            GRAPH,
            &request(),
            Some("other-actor"),
            nodes,
            transitions,
            DurableCrypto::none(),
            1_001,
        )?;
        assert_eq!(collision.outcome, ControlLeaseIssueOutcome::Collision);
        assert!(collision.approved.is_none());
        Ok(())
    });
}
