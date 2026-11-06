//! Workflow registry read proofs (MEDULLA-DECISION-ENGINE-R166.3, R166.4).

use super::read_from_core;
use medulla_graph_core::graph::GraphCore;
use medulla_work_types::orchestration::workflows::{
    WorkflowIdentity, WorkflowLifecycle, WorkflowRegistrationEntry, WorkflowRegistryError,
};

fn identity() -> WorkflowIdentity {
    WorkflowIdentity {
        workflow_id: "summarize".into(),
        version: "1.2.0".into(),
        content_digest: format!("sha256:{}", "a".repeat(64)),
    }
}

fn seeded(scope: &str) -> GraphCore {
    let core = GraphCore::new();
    let entry = WorkflowRegistrationEntry {
        identity: identity(),
        owner_scope: scope.into(),
        lifecycle: WorkflowLifecycle::Active,
    };
    core.add_node(
        WorkflowRegistrationEntry::node_id(&entry.identity),
        rmp_serde::to_vec_named(&entry.node_properties("tenant-a")).unwrap(),
    );
    core
}

// spec: MEDULLA-DECISION-ENGINE-R166.3
#[test]
fn read_returns_the_stored_registration_to_its_owner_only() {
    let core = seeded("scope-a");
    let read = read_from_core(&core, "scope-a", &identity()).unwrap();
    assert_eq!(read.identity, identity());
    assert_eq!(read.lifecycle, WorkflowLifecycle::Active);
    assert_eq!(
        read_from_core(&core, "scope-b", &identity()),
        Err(WorkflowRegistryError::Unauthorized)
    );
}

// spec: MEDULLA-DECISION-ENGINE-R166.4
#[test]
fn read_of_an_unknown_identity_refuses_and_returns_no_default() {
    let core = GraphCore::new();
    assert_eq!(
        read_from_core(&core, "scope-a", &identity()),
        Err(WorkflowRegistryError::IdentityUnknown)
    );
    let mut other = identity();
    other.version = "9.9.9".into();
    assert_eq!(
        read_from_core(&seeded("scope-a"), "scope-a", &other),
        Err(WorkflowRegistryError::IdentityUnknown)
    );
}
