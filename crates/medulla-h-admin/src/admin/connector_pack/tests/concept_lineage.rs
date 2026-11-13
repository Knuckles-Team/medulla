//! Served acceptance of `Method::ConceptLineage` through the real signed
//! dispatch: claims, edges, reads and every refusal by registered code.

use super::{context, ok, refused, Served, TENANT};
use medulla_agent_types::concept_lineage::{
    ConceptEdgeRecord, ConceptLineageOp, ConceptReadResult, ConceptRecord,
};
use medulla_protocol_types::protocol::{Method, Response};

async fn lineage(served: &Served, key: &str, op: ConceptLineageOp) -> Response {
    served
        .call(key, Method::ConceptLineage { op: Box::new(op) })
        .await
}

async fn claim(served: &Served, key: &str, id: &str, repository: &str) -> Response {
    lineage(
        served,
        key,
        ConceptLineageOp::Claim {
            context: context(key),
            concept_id: id.to_string(),
            repository: repository.to_string(),
        },
    )
    .await
}

async fn edge(served: &Served, key: &str, from: &str, to: &str) -> Response {
    lineage(
        served,
        key,
        ConceptLineageOp::AddEdge {
            context: context(key),
            superseded: from.to_string(),
            successor: to.to_string(),
            repository: "medulla".to_string(),
        },
    )
    .await
}

async fn read(served: &Served, key: &str, id: &str) -> ConceptReadResult {
    ok(
        "Read",
        lineage(
            served,
            key,
            ConceptLineageOp::Read {
                tenant_id: TENANT.to_string(),
                concept_id: id.to_string(),
            },
        )
        .await,
    )
}

// spec: MEDULLA-DURABLE-KERNEL-R094.7.1
#[tokio::test]
async fn claimed_ids_and_edges_are_served_and_read_back() {
    let served = Served::new();
    let (a, b) = ("MEDULLA-KG.memory", "MEDULLA-KG.recall");
    let first: ConceptRecord = ok("claim a", claim(&served, "cl-a", a, "medulla").await);
    assert_eq!(first.concept_id, a);
    assert_eq!(first.repository, "medulla");
    let again: ConceptRecord = ok("re-claim a", claim(&served, "cl-a2", a, "medulla").await);
    assert_eq!(
        again, first,
        "a re-claim by the owner keeps its first evidence"
    );
    let _: ConceptRecord = ok("claim b", claim(&served, "cl-b", b, "medulla").await);
    let added: ConceptEdgeRecord = ok("edge", edge(&served, "cl-e", a, b).await);
    assert_eq!(
        (added.superseded.as_str(), added.successor.as_str()),
        (a, b)
    );

    let seen = read(&served, "cl-read-a", a).await;
    assert_eq!(seen.concept, Some(first));
    assert_eq!(seen.successors, vec![added]);
    assert!(!seen.truncated);
    let leaf = read(&served, "cl-read-b", b).await;
    assert!(leaf.concept.is_some() && leaf.successors.is_empty());
    let absent = read(&served, "cl-read-x", "MEDULLA-KG.never-claimed").await;
    assert!(absent.concept.is_none() && absent.successors.is_empty());
}

// spec: MEDULLA-DURABLE-KERNEL-R094.7.1
#[tokio::test]
async fn another_repository_claiming_a_path_is_refused_by_code() {
    let served = Served::new();
    let id = "MEDULLA-KG.memory.paging";
    let _: ConceptRecord = ok("claim", claim(&served, "col-1", id, "medulla").await);
    let error = refused("collision", claim(&served, "col-2", id, "meristem").await);
    assert!(error.starts_with("CONCEPT_ID_PATH_COLLISION"), "{error}");
    let malformed = refused(
        "malformed",
        claim(&served, "col-3", "medulla.bad", "x").await,
    );
    assert!(malformed.starts_with("CONCEPT_ID_MALFORMED"), "{malformed}");
    assert_eq!(
        read(&served, "col-read", id)
            .await
            .concept
            .unwrap()
            .repository,
        "medulla"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R094.7
#[tokio::test]
async fn lineage_refuses_self_unknown_and_cycle_edges_by_code() {
    let served = Served::new();
    let ids = ["MEDULLA-KG.one", "MEDULLA-KG.two", "MEDULLA-KG.three"];
    for (n, id) in ids.iter().enumerate() {
        let _: ConceptRecord = ok(
            "claim",
            claim(&served, &format!("lin-c{n}"), id, "medulla").await,
        );
    }
    let _: ConceptEdgeRecord = ok("one->two", edge(&served, "lin-e1", ids[0], ids[1]).await);
    let _: ConceptEdgeRecord = ok("two->three", edge(&served, "lin-e2", ids[1], ids[2]).await);

    let cycle = refused("cycle", edge(&served, "lin-e3", ids[2], ids[0]).await);
    assert!(cycle.starts_with("CONCEPT_LINEAGE_CYCLE"), "{cycle}");
    let selfie = refused("self", edge(&served, "lin-e4", ids[0], ids[0]).await);
    assert!(selfie.starts_with("CONCEPT_LINEAGE_SELF_EDGE"), "{selfie}");
    let unknown = refused(
        "unknown",
        edge(&served, "lin-e5", ids[0], "MEDULLA-KG.ghost").await,
    );
    assert!(
        unknown.starts_with("CONCEPT_LINEAGE_UNKNOWN_ID"),
        "{unknown}"
    );
    assert_eq!(read(&served, "lin-read", ids[2]).await.successors.len(), 0);
    assert_eq!(read(&served, "lin-read2", ids[0]).await.successors.len(), 1);
}

// spec: MEDULLA-DURABLE-KERNEL-R094.7.1
#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let served = Served::new();
    let mut shell = context("tenant-x");
    shell.tenant_id = "tenant-other".to_string();
    let error = refused(
        "cross-tenant claim",
        lineage(
            &served,
            "tenant-x",
            ConceptLineageOp::Claim {
                context: shell,
                concept_id: "MEDULLA-KG.memory".to_string(),
                repository: "medulla".to_string(),
            },
        )
        .await,
    );
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
    let error = refused(
        "cross-tenant read",
        lineage(
            &served,
            "tenant-y",
            ConceptLineageOp::Read {
                tenant_id: "tenant-other".to_string(),
                concept_id: "MEDULLA-KG.memory".to_string(),
            },
        )
        .await,
    );
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
