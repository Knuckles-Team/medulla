//! Package runs recorded through `DecisionLog` `Learn`
//! (MEDULLA-DECISION-ENGINE-R160.3): a run is written once under the tenant's
//! `package_run/<package digest>/<run id>` key, an equal replay answers the
//! stored row and the same key with another verdict is an
//! `IDEMPOTENCY_CONFLICT`. Another tenant never reads the row.

use super::retrieval_tests::learn;
use super::*;
use medulla_decision_types::decision::package_validation::{
    package_run_key, package_run_prefix, RunHistoryEntry,
};
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_types::contract::{Digest256, OpaqueId};

fn run(id: &str, passed: bool) -> RunHistoryEntry {
    RunHistoryEntry {
        run_id: OpaqueId::new(id).unwrap(),
        package_digest: Digest256::sha256(b"package"),
        passed,
    }
}

async fn send(h: &Harness, entry: RunHistoryEntry) -> Result<RunHistoryEntry, String> {
    match learn(
        h,
        "decider",
        LearningWrite::PackageRun {
            run: Box::new(entry),
        },
    )
    .await
    {
        Ok(LearningRecorded::PackageRun(stored)) => Ok(*stored),
        Ok(_) => Err("not a package run".to_string()),
        Err(error) => Err(error),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R160.3
#[tokio::test]
async fn package_run_is_written_once() {
    let h = Harness::new().await;
    let digest = Digest256::sha256(b"package");
    let rows = |tenant: &str| {
        h.store
            .decision_artifacts_with_prefix(tenant, &package_run_prefix(&digest), 100)
            .unwrap()
    };

    // The first write stores the row under the shared key.
    let first = send(&h, run("run-a", true)).await.unwrap();
    assert!(first.passed);
    let stored = rows(TENANT);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].0,
        package_run_key(&digest, &OpaqueId::new("run-a").unwrap())
    );

    // An equal replay answers the stored row and adds none.
    assert_eq!(send(&h, run("run-a", true)).await.unwrap(), first);
    assert_eq!(rows(TENANT).len(), 1);

    // The same key with another verdict is a conflict and changes nothing.
    let error = send(&h, run("run-a", false)).await.unwrap_err();
    assert!(error.starts_with("IDEMPOTENCY_CONFLICT"), "{error}");
    assert_eq!(rows(TENANT), stored);

    // A second run adds a second row, in ascending run id order.
    send(&h, run("run-b", false)).await.unwrap();
    let keys: Vec<String> = rows(TENANT).into_iter().map(|(key, _)| key).collect();
    assert_eq!(
        keys,
        vec![
            package_run_key(&digest, &OpaqueId::new("run-a").unwrap()),
            package_run_key(&digest, &OpaqueId::new("run-b").unwrap()),
        ]
    );

    // Another tenant holds none of these rows.
    assert!(rows("tenant-other").is_empty());
}
