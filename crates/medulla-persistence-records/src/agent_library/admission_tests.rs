//! MEDULLA-DECISION-ENGINE-R036.104: the Agent Library admission participant.

use super::admission::{
    AdmissionEvent, AdmissionIntent, AdmissionState, ComponentHead, ADMISSION_ABORTED,
    ADMISSION_COMPONENT_WITHDRAWN, ADMISSION_HEAD_CHANGED, ADMISSION_INTENT_CHANGED,
    ADMISSION_NOT_RESERVED, ADMISSION_WRITE_BLOCKED,
};
use super::AgentLibraryStore;
use crate::agent_fixtures::{mutation_context, open_agent_store};
use crate::decision_record::tests::{decided, seed_library, TENANT};
use medulla_agent_types::agent_component::AgentComponentRetireRequest;

fn heads(store: &AgentLibraryStore) -> Vec<ComponentHead> {
    let mut heads: Vec<ComponentHead> = decided(store)
        .record
        .inputs
        .candidates
        .iter()
        .map(|candidate| ComponentHead {
            component_id: candidate.component_id.clone(),
            entry_revision: candidate.entry_revision,
            definition_digest: candidate.definition_digest.clone(),
        })
        .collect();
    heads.sort_by(|left, right| left.component_id.cmp(&right.component_id));
    heads
}

fn intent(txn: &str, write_key: &str, heads: Vec<ComponentHead>) -> AdmissionIntent {
    AdmissionIntent {
        txn_id: txn.into(),
        profile_revision: "policy-v1".into(),
        component_heads: heads,
        visibility_digest: format!("sha256:{}", "a".repeat(64)),
        write_key: write_key.into(),
    }
}

fn retire(store: &AgentLibraryStore, head: &ComponentHead, nonce: u8) -> Result<(), String> {
    store
        .retire_component(AgentComponentRetireRequest {
            context: mutation_context(
                store,
                TENANT,
                &format!("retire:{}:{nonce}", head.component_id),
                nonce,
                head.entry_revision,
                "agent-component:retire",
            ),
            component_id: head.component_id.clone(),
        })
        .map(|_| ())
}

// spec: MEDULLA-DECISION-ENGINE-R036.104
#[test]
fn admission_participant_fences_and_replays() {
    let (dir, store) = open_agent_store();
    seed_library(&store);
    let heads = heads(&store);
    assert!(heads.len() >= 2);
    let first = intent("txn-1", "write-1", heads.clone());

    // Reserve, then an exact retry succeeds with the same state.
    assert_eq!(
        store.reserve_admission(TENANT, &first).unwrap(),
        AdmissionState::Reserved
    );
    assert_eq!(
        store.reserve_admission(TENANT, &first).unwrap(),
        AdmissionState::Reserved
    );
    assert_eq!(first.premises().len(), heads.len());
    assert!(first.premises()[0].starts_with("component_head:"));

    // A changed intent for the same txn refuses and changes nothing.
    let mut changed = first.clone();
    changed.visibility_digest = format!("sha256:{}", "b".repeat(64));
    let error = store.reserve_admission(TENANT, &changed).unwrap_err();
    assert!(error.starts_with(ADMISSION_INTENT_CHANGED), "{error}");

    // A writer of a fenced component is blocked; so is a second holder of the write key.
    let error = retire(&store, &heads[0], 41).unwrap_err();
    assert!(error.starts_with(ADMISSION_WRITE_BLOCKED), "{error}");
    let rival = intent("txn-2", "write-1", heads.clone());
    let error = store.reserve_admission(TENANT, &rival).unwrap_err();
    assert!(error.starts_with(ADMISSION_WRITE_BLOCKED), "{error}");
    assert!(store.admission(TENANT, "txn-2").unwrap().is_none());

    // The reservation survives a reopen and still fences.
    let path = dir.path().to_str().unwrap().to_string();
    drop(store);
    let store = AgentLibraryStore::open(&path).unwrap();
    assert_eq!(
        store.admission(TENANT, "txn-1").unwrap().unwrap().state,
        AdmissionState::Reserved
    );
    let error = retire(&store, &heads[0], 42).unwrap_err();
    assert!(error.starts_with(ADMISSION_WRITE_BLOCKED), "{error}");

    // COMMIT releases the fence; a repeat replays; the opposite decision is refused by name.
    assert_eq!(
        store
            .resolve_admission(TENANT, "txn-1", AdmissionEvent::Commit)
            .unwrap(),
        AdmissionState::Committed
    );
    assert_eq!(
        store
            .resolve_admission(TENANT, "txn-1", AdmissionEvent::Commit)
            .unwrap(),
        AdmissionState::Committed
    );
    let error = store
        .resolve_admission(TENANT, "txn-1", AdmissionEvent::Abort)
        .unwrap_err();
    assert!(
        error.contains("TRANSITION_REFUSED")
            && error.contains("ReservationMachine")
            && error.contains("committed")
            && error.contains("Abort"),
        "{error}"
    );
    // An exact retry of a committed intent replays without a new fence.
    assert_eq!(
        store.reserve_admission(TENANT, &first).unwrap(),
        AdmissionState::Committed
    );

    // A stale head refuses; an absent or retired component is withdrawn.
    let mut stale = heads.clone();
    stale[0].entry_revision += 90;
    let error = store
        .reserve_admission(TENANT, &intent("txn-3", "write-3", stale))
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_HEAD_CHANGED), "{error}");
    let mut ghost = heads.clone();
    ghost[0].component_id = "ghost-component".into();
    let error = store
        .reserve_admission(TENANT, &intent("txn-4", "write-4", ghost))
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_COMPONENT_WITHDRAWN), "{error}");
    retire(&store, &heads[1], 43).expect("the released fence lets the writer through");
    let error = store
        .reserve_admission(TENANT, &intent("txn-5", "write-5", heads.clone()))
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_COMPONENT_WITHDRAWN), "{error}");

    // ABORT: the same lifecycle, the other terminal state.
    let live = vec![heads[0].clone()];
    let abortable = intent("txn-6", "write-6", live);
    store.reserve_admission(TENANT, &abortable).unwrap();
    assert_eq!(
        store
            .resolve_admission(TENANT, "txn-6", AdmissionEvent::Abort)
            .unwrap(),
        AdmissionState::Aborted
    );
    assert_eq!(
        store
            .resolve_admission(TENANT, "txn-6", AdmissionEvent::Abort)
            .unwrap(),
        AdmissionState::Aborted
    );
    assert!(store
        .resolve_admission(TENANT, "txn-6", AdmissionEvent::Commit)
        .unwrap_err()
        .contains("TRANSITION_REFUSED"));
    let error = store.reserve_admission(TENANT, &abortable).unwrap_err();
    assert!(error.starts_with(ADMISSION_ABORTED), "{error}");
    let error = store
        .resolve_admission(TENANT, "txn-unknown", AdmissionEvent::Commit)
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_NOT_RESERVED), "{error}");
    retire(&store, &heads[0], 44).expect("an aborted reservation fences nothing");
}
