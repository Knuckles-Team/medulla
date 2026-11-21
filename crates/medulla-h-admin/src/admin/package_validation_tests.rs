//! `Method::PublishSkillPackage` served over the recorded package runs
//! (MEDULLA-DECISION-ENGINE-R160.3): runs recorded with `DecisionLog` `learn`
//! (`LearningWrite::PackageRun`), the publication receipt, its replay, the
//! refusals that write nothing and the tenant boundary of the run history.

use medulla_catalog_types::fleet_catalog::rendered::{
    RenderedSkillPackage, RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::package_validation::{
    package_publication_key, package_run_key, PublicationReceipt, PublishSkillPackageRequest,
    RunHistoryEntry,
};
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::{Digest256, OpaqueId};

use super::connector_pack::tests::{ok, Served, TENANT};

fn package(content: &[u8]) -> RenderedSkillPackage {
    RenderedSkillPackage {
        schema_version: RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
        uri: "skill://triage".to_string(),
        digest: Digest256::sha256(content).to_hex(),
        ontology_refs: Vec::new(),
        graph_revision: 1,
    }
}

fn run(content: &[u8], id: &str, passed: bool) -> RunHistoryEntry {
    RunHistoryEntry {
        run_id: OpaqueId::new(id).unwrap(),
        package_digest: Digest256::sha256(content),
        passed,
    }
}

async fn record(served: &Served, key: &str, entry: RunHistoryEntry) -> LearningRecorded {
    ok(
        "record package run",
        served
            .call(
                key,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Learn {
                        tenant_id: TENANT.to_string(),
                        write: LearningWrite::PackageRun {
                            run: Box::new(entry),
                        },
                    }),
                },
            )
            .await,
    )
}

async fn publish(
    served: &Served,
    key: &str,
    id: &str,
    package: RenderedSkillPackage,
) -> medulla_protocol_types::protocol::Response {
    served
        .call(
            key,
            Method::PublishSkillPackage {
                request: Box::new(PublishSkillPackageRequest {
                    package_id: OpaqueId::new(id).unwrap(),
                    package,
                }),
            },
        )
        .await
}

// spec: MEDULLA-DECISION-ENGINE-R160.3
#[tokio::test]
async fn package_validation_routes() {
    let served = Served::new();
    let store = served.state.write().await.ensure_agent_library().unwrap();

    // Two passing runs are recorded through `DecisionLog` `learn`; an equal
    // replay answers the stored row.
    let content = b"package one";
    for (key, id) in [("run:a", "run-a"), ("run:b", "run-b")] {
        let LearningRecorded::PackageRun(stored) = record(&served, key, run(content, id, true)).await
        else {
            panic!("a package run answers its stored row");
        };
        assert!(stored.passed);
    }
    record(&served, "run:a:replay", run(content, "run-a", true)).await;

    // Publishing answers the receipt over the recorded history.
    let receipt: PublicationReceipt = ok(
        "publish",
        publish(&served, "publish:1", "package-one", package(content)).await,
    );
    assert_eq!(receipt.package_id.as_str(), "package-one");
    assert!(receipt.validation_result.validation_passed);
    assert_eq!(
        receipt.validation_result.rendered_digest,
        Digest256::sha256(content)
    );
    // The replay answers the stored receipt, byte for byte, and writes nothing.
    let replayed: PublicationReceipt = ok(
        "replay",
        publish(&served, "publish:2", "package-one", package(content)).await,
    );
    assert_eq!(replayed, receipt);
    assert!(store
        .decision_artifact(
            TENANT,
            &package_publication_key(&Digest256::sha256(content))
        )
        .unwrap()
        .is_some());

    // A digest with no recorded runs refuses and writes no receipt.
    let unpublished = package(b"package two");
    let refused = publish(&served, "publish:3", "package-two", unpublished.clone())
        .await
        .error
        .expect("a package with no runs is refused");
    assert!(refused.starts_with("PACKAGE_RUN_HISTORY_MISSING"), "{refused}");
    assert!(store
        .decision_artifact(
            TENANT,
            &package_publication_key(&Digest256::sha256(b"package two"))
        )
        .unwrap()
        .is_none());

    // A failing run refuses validation.
    let failing = b"package three";
    record(&served, "run:c", run(failing, "run-c", false)).await;
    let refused = publish(&served, "publish:4", "package-three", package(failing))
        .await
        .error
        .expect("a failing history is refused");
    assert!(refused.starts_with("PACKAGE_VALIDATION_FAILED"), "{refused}");

    // A digest that does not parse refuses first.
    let mut broken = package(content);
    broken.digest = "not-a-digest".to_string();
    let refused = publish(&served, "publish:5", "package-broken", broken)
        .await
        .error
        .expect("an unparseable digest is refused");
    assert!(refused.starts_with("PACKAGE_DIGEST_MISMATCH"), "{refused}");

    // Another tenant's runs are never read.
    let theirs = b"package four";
    let entry = run(theirs, "run-d", true);
    store
        .put_decision_artifacts(
            "tenant-other",
            &[(
                package_run_key(&entry.package_digest, &entry.run_id),
                encode_artifact(&entry).unwrap(),
            )],
        )
        .unwrap();
    let refused = publish(&served, "publish:6", "package-four", package(theirs))
        .await
        .error
        .expect("another tenant's runs do not count");
    assert!(refused.starts_with("PACKAGE_RUN_HISTORY_MISSING"), "{refused}");
}
