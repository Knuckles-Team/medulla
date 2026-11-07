//! The store side of the WorkItem machine (MEDULLA-ORCH-VOCAB-R812.2): every writer of a
//! WorkItem row's `status` goes through [`fire`], which decodes the stored state, runs
//! `WorkItemStatus::apply` and stores the state it returns. No writer inserts a status
//! literal, and no writer keeps its own list of terminal states.

use medulla_work_types::work_item_machine::{WorkItemEvent, WorkItemFacts, WorkItemStatus};

use super::*;

/// The stored `status` of a row. An unknown text, such as the retired `completed`, is a
/// typed decode error.
pub fn stored_status(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<WorkItemStatus, String> {
    WorkItemStatus::decode_stored(property_string(props, "status"))
        .map_err(|error| error.to_string())
}

/// Whether the row's stored state is terminal. An unknown text is not a terminal state.
pub fn is_terminal_row(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    WorkItemStatus::from_stored(property_string(props, "status"))
        .is_some_and(WorkItemStatus::is_terminal)
}

/// Whether the row's stored state holds a lease (Leased or Running).
pub fn holds_lease_row(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    WorkItemStatus::from_stored(property_string(props, "status"))
        .is_some_and(WorkItemStatus::holds_lease)
}

/// The guard facts a row carries by itself. `lease_live` is the caller's own comparison of
/// `lease_expires_at` with its clock; `fence_matches` is false until the caller proves it.
pub fn row_facts(
    props: &serde_json::Map<String, serde_json::Value>,
    now_s: f64,
    lease_live: bool,
) -> WorkItemFacts {
    WorkItemFacts {
        open_dependencies: property_u64(props, "dep_count"),
        retry_due: property_f64(props, "next_retry_at") <= now_s,
        attempts: property_u64(props, "attempt"),
        max_attempts: property_u64(props, "max_attempts").max(1),
        effect_started: super::effect::effect_started(props),
        fence_matches: false,
        lease_live,
        counters_ok: property_u64(props, "lease_epoch").checked_add(1).is_some()
            && property_u64(props, "fencing_token")
                .checked_add(1)
                .is_some(),
    }
}

/// Fire `event` on the row: apply the machine to the stored state and store the state
/// it returns. A refusal leaves the row unchanged and is returned as the typed
/// `TRANSITION_REFUSED` text.
pub fn fire(
    props: &mut serde_json::Map<String, serde_json::Value>,
    event: WorkItemEvent,
    facts: &WorkItemFacts,
) -> Result<WorkItemStatus, String> {
    let from = stored_status(props)?;
    let to = from
        .apply(event, facts)
        .map_err(|refusal| refusal.to_string())?;
    props.insert(
        "status".into(),
        serde_json::Value::String(to.as_stored().into()),
    );
    Ok(to)
}

#[cfg(test)]
#[path = "tests/machine_cutover.rs"]
mod machine_cutover;
