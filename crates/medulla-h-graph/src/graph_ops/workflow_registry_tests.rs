//! Workflow registry write proofs (MEDULLA-DECISION-ENGINE-R166.3, R166.4).

use super::workflow_registry::{admit_registration, Admitted, REFUSALS};
use medulla_graph_core::graph::GraphCore;
use medulla_server_mutation::mutation::apply_coalescable_write;
use medulla_types::contract::ServerErrorCode;
use medulla_work_types::orchestration::workflows::{
    read_registration, WorkflowIdentity, WorkflowLifecycle, WorkflowRegistrationEntry,
    WorkflowRegistryError,
};

const TENANT: &str = "tenant-a";

fn identity() -> WorkflowIdentity {
    WorkflowIdentity {
        workflow_id: "summarize".into(),
        version: "1.2.0".into(),
        content_digest: format!("sha256:{}", "a".repeat(64)),
    }
}

fn registration(identity: WorkflowIdentity) -> WorkflowRegistrationEntry {
    WorkflowRegistrationEntry {
        identity,
        owner_scope: String::new(),
        lifecycle: WorkflowLifecycle::Active,
        steps: Vec::new(),
    }
}

fn stored(
    core: &GraphCore,
    identity: &WorkflowIdentity,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    core.get_node_properties(&WorkflowRegistrationEntry::node_id(identity))
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
}

// spec: MEDULLA-DECISION-ENGINE-R166
// spec: MEDULLA-DECISION-ENGINE-R166.3
#[test]
fn register_read_and_refuse_unknown() {
    let core = GraphCore::new();
    let request = registration(identity());
    let Admitted::Write(write, entry) =
        admit_registration(&core, TENANT, "scope-a", &request).unwrap()
    else {
        panic!("a first registration writes");
    };
    apply_coalescable_write(&core, &write).unwrap();

    let read =
        read_registration(stored(&core, &identity()).as_ref(), "scope-a", &identity()).unwrap();
    assert_eq!(read, entry);
    assert_eq!(read.identity.workflow_id, "summarize");
    assert_eq!(read.identity.version, "1.2.0");
    assert_eq!(read.identity.content_digest, identity().content_digest);
    assert_eq!(read.owner_scope, "scope-a");

    // A second tenant cannot read it.
    assert_eq!(
        read_registration(stored(&core, &identity()).as_ref(), "scope-b", &identity()),
        Err(WorkflowRegistryError::Unauthorized)
    );
    // Registration is idempotent.
    match admit_registration(&core, TENANT, "scope-a", &request).unwrap() {
        Admitted::Same(again) => assert_eq!(again, entry),
        Admitted::Write(..) => panic!("an equal registration commits nothing"),
    }
    // An unknown identity refuses.
    let mut unknown = identity();
    unknown.version = "2.0.0".into();
    assert_eq!(
        read_registration(stored(&core, &unknown).as_ref(), "scope-a", &unknown),
        Err(WorkflowRegistryError::IdentityUnknown)
    );
}

// spec: MEDULLA-ORCH-VOCAB-R815.9
#[test]
fn reactivating_a_withdrawn_workflow_refuses() {
    let core = GraphCore::new();
    // A first registration must be active.
    let mut first = registration(identity());
    first.lifecycle = WorkflowLifecycle::Stale;
    let refused = admit_registration(&core, TENANT, "scope-a", &first).unwrap_err();
    assert_eq!(refused, "INVALID_ARGUMENT: a first registration is active");
    assert_eq!(core.node_count(), 0);

    let Admitted::Write(write, _) =
        admit_registration(&core, TENANT, "scope-a", &registration(identity())).unwrap()
    else {
        panic!("a first registration writes");
    };
    apply_coalescable_write(&core, &write).unwrap();

    let mut withdrawn = registration(identity());
    withdrawn.lifecycle = WorkflowLifecycle::Withdrawn;
    let Admitted::Write(write, _) =
        admit_registration(&core, TENANT, "scope-a", &withdrawn).unwrap()
    else {
        panic!("withdrawing an active workflow writes");
    };
    apply_coalescable_write(&core, &write).unwrap();

    let refused =
        admit_registration(&core, TENANT, "scope-a", &registration(identity())).unwrap_err();
    assert!(refused.starts_with("TRANSITION_REFUSED: "), "{refused}");
    assert!(refused.contains("terminal_state"), "{refused}");
    let held = stored(&core, &identity()).expect("the node stays");
    assert_eq!(held["lifecycle"], "withdrawn");
}

// spec: MEDULLA-DECISION-ENGINE-R166.4
#[test]
fn unknown_identity_refuses_without_default() {
    let core = GraphCore::new();
    let mut no_id = identity();
    no_id.workflow_id.clear();
    let mut no_version = identity();
    no_version.version.clear();
    let mut bad_digest = identity();
    bad_digest.content_digest = "md5:zz".into();
    for (bad, code) in [
        (no_id, "WORKFLOW_ID_MISSING"),
        (no_version, "WORKFLOW_VERSION_MISSING"),
        (bad_digest, "WORKFLOW_DIGEST_INVALID"),
    ] {
        let refused = admit_registration(&core, TENANT, "scope-a", &registration(bad)).unwrap_err();
        assert!(refused.starts_with(code), "{refused}");
    }
    assert_eq!(core.node_count(), 0, "a refused register writes nothing");

    let mut foreign = registration(identity());
    foreign.owner_scope = "scope-b".into();
    let refused = admit_registration(&core, TENANT, "scope-a", &foreign).unwrap_err();
    assert!(refused.starts_with("WORKFLOW_UNAUTHORIZED"), "{refused}");

    assert_eq!(
        read_registration(None, "scope-a", &identity()),
        Err(WorkflowRegistryError::IdentityUnknown)
    );
    assert_eq!(
        WorkflowRegistryError::IdentityUnknown.code(),
        "WORKFLOW_IDENTITY_UNKNOWN"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R166.4
#[test]
fn codes_are_unique_and_stable() {
    let mut sorted = REFUSALS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), REFUSALS.len());
    for code in REFUSALS {
        assert!(
            ServerErrorCode::ALL
                .iter()
                .any(|item| item.as_str() == *code),
            "{code} is not a ServerErrorCode"
        );
    }
}
