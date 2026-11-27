//! The governed pointer store (EH-396, EH-397): one compare-and-set row per
//! pointer in the Agent Library control owner, under the verified carrier's
//! tenant scope. Activation pushes the previous target onto the rollback stack;
//! rollback pops it; both append an audited event. A pointer that moved since
//! it was read is refused, never overwritten.

use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::{
    decision::statistical::retrieval_pointer::PointerEvent,
    decision::statistical::retrieval_pointer::PointerState,
    decision::statistical::retrieval_pointer::PointerTransition,
    decision::statistical::retrieval_pointer::MAX_POINTER_HISTORY,
};
use medulla_types::contract::BoundedVec;

use super::stat_support::refusal;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{
    decode_artifact, encode_artifact, pointer_artifact_key, read_pointer,
};

/// Who moves which pointer, and when.
pub(super) struct PointerMove<'a> {
    pub(super) tenant: &'a str,
    pub(super) key: String,
    pub(super) principal: &'a str,
    pub(super) now_ms: u64,
}

/// A target and the receipt that qualified it.
pub(super) struct Qualified {
    pub(super) target: String,
    pub(super) receipt_digest: String,
}

/// Most pointers one tenant's relation lists.
const MAX_POINTERS: usize = 4_096;

fn bounded_push(
    list: &BoundedVec<PointerEvent, MAX_POINTER_HISTORY>,
    event: PointerEvent,
) -> Result<BoundedVec<PointerEvent, MAX_POINTER_HISTORY>, String> {
    let mut events: Vec<PointerEvent> = list.iter().cloned().collect();
    events.push(event);
    let excess = events.len().saturating_sub(MAX_POINTER_HISTORY);
    BoundedVec::new(events.split_off(excess))
}

fn write(
    store: &AgentLibraryStore,
    at: &PointerMove,
    expected: Option<Vec<u8>>,
    state: &PointerState,
) -> Result<(), String> {
    let key = pointer_artifact_key(&at.key);
    store.swap_decision_artifact(at.tenant, &key, expected, encode_artifact(state)?)
}

fn event(at: &PointerMove, transition: PointerTransition, to: Option<&Qualified>) -> PointerEvent {
    PointerEvent {
        transition,
        target: to.map(|q| q.target.clone()),
        receipt_digest: to.map(|q| q.receipt_digest.clone()),
        principal: at.principal.to_string(),
        at_ms: at.now_ms,
    }
}

/// Point at a qualified target. Re-activating the active target is a replay.
pub(super) fn activate(
    store: &AgentLibraryStore,
    at: &PointerMove,
    to: &Qualified,
) -> Result<PointerState, String> {
    let (expected, state) = read_pointer(store, at.tenant, &at.key)?;
    if state.target() == Some(to.target.as_str()) {
        return Ok(state);
    }
    let activated = event(at, PointerTransition::Activated, Some(to));
    let stack = match state.active.clone() {
        Some(previous) => bounded_push(&state.stack, previous)?,
        None => state.stack.clone(),
    };
    let next = PointerState {
        key: at.key.clone(),
        active: Some(activated.clone()),
        stack,
        history: bounded_push(&state.history, activated)?,
    };
    write(store, at, expected, &next)?;
    record_champion(store, at, to)?;
    Ok(next)
}

/// The revision row of a champion move (R817): subjects of `champion:` keys
/// list their revisions; other pointers are not revisions.
fn record_champion(
    store: &AgentLibraryStore,
    at: &PointerMove,
    to: &Qualified,
) -> Result<(), String> {
    let Some((subject_kind, subject_ref)) = RevisionRow::subject_of(&at.key) else {
        return Ok(());
    };
    super::stat_proposals::record_revision(
        store,
        at.tenant,
        &RevisionRow {
            id: to.target.clone(),
            subject_kind,
            subject_ref,
            stage: RevisionStage::Champion,
            digest: to.receipt_digest.clone(),
            evaluation_receipt_id: Some(to.receipt_digest.clone()),
            created_ms: at.now_ms,
        },
    )
}

/// Return to the target active before the current one (or to the default
/// when there was none).
pub(super) fn rollback(
    store: &AgentLibraryStore,
    at: &PointerMove,
) -> Result<PointerState, String> {
    let (expected, state) = read_pointer(store, at.tenant, &at.key)?;
    if state.active.is_none() {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "nothing is active behind this pointer",
        ));
    }
    let mut stack: Vec<PointerEvent> = state.stack.iter().cloned().collect();
    let restored = stack.pop().map(|previous| PointerEvent {
        transition: PointerTransition::RolledBack,
        principal: at.principal.to_string(),
        at_ms: at.now_ms,
        ..previous
    });
    let logged = restored
        .clone()
        .unwrap_or_else(|| event(at, PointerTransition::RolledBack, None));
    let next = PointerState {
        key: at.key.clone(),
        active: restored,
        stack: BoundedVec::new(stack)?,
        history: bounded_push(&state.history, logged)?,
    };
    write(store, at, expected, &next)?;
    Ok(next)
}

/// Every pointer of `tenant` as `(key, seq, active, event)` rows: each
/// pointer's audited history in order, the active move flagged.
pub(super) fn pointer_events(
    store: &AgentLibraryStore,
    tenant: &str,
) -> Result<Vec<(String, usize, bool, PointerEvent)>, String> {
    let mut rows = Vec::new();
    for (_, bytes) in store.decision_artifacts_with_prefix(tenant, "pointer:", MAX_POINTERS)? {
        let state: PointerState = decode_artifact(&bytes, "pointer state")?;
        let last = state.history.len().saturating_sub(1);
        let active = state.active.is_some();
        for (seq, event) in state.history.iter().enumerate() {
            rows.push((state.key.clone(), seq, active && seq == last, event.clone()));
        }
    }
    Ok(rows)
}

/// The champion `pointer_key` names (MEDULLA-DECISION-ENGINE-R805.13): the
/// active revision, the revision it replaced and the decision that promoted
/// it. A key that is no subject's, or whose pointer holds no active revision,
/// refuses `POINTER_NOT_FOUND`; another tenant's pointer reads as absent.
/// Writes nothing.
pub(super) fn champion_pointer(
    store: &AgentLibraryStore,
    tenant: &str,
    pointer_key: &str,
) -> Result<medulla_decision_outcome_types::decision::promotion::ChampionPointer, String> {
    use medulla_decision_outcome_types::decision::promotion::ChampionPointer;
    use medulla_decision_types::decision::held_out::RevisionSubject;
    use medulla_types::contract::OpaqueId;
    let not_found = || {
        format!(
            "{}: no champion is active under `{pointer_key}`",
            medulla_decision_types::decision::codes::POINTER_NOT_FOUND.as_str()
        )
    };
    let opaque = |value: &str| OpaqueId::new(value.to_owned()).map_err(|_| not_found());
    let subject = RevisionSubject::from_pointer_key(pointer_key).ok_or_else(not_found)?;
    let (_, state) = read_pointer(store, tenant, pointer_key)?;
    let active = state.active.as_ref().ok_or_else(not_found)?;
    let target = active.target.as_deref().ok_or_else(not_found)?;
    let receipt = active.receipt_digest.as_deref().ok_or_else(not_found)?;
    let parent_ref = match state.stack.as_slice().last().and_then(|event| event.target.as_deref()) {
        Some(parent) => Some(opaque(parent)?),
        None => None,
    };
    // A skill champion carries the canary split in force, when there is one.
    let canary = match &subject {
        RevisionSubject::Skill { skill_ref, .. } => {
            super::stat_skill_ablation::read_canary(store, tenant, skill_ref)?
        }
        _ => None,
    };
    Ok(ChampionPointer {
        subject,
        revision_ref: opaque(target)?,
        parent_ref,
        promoted_by_decision: opaque(&format!("decision:{receipt}"))?,
        canary,
    })
}
