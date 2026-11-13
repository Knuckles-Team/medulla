//! `Method::RevisionProposalList` (MEDULLA-DECISION-ENGINE-R817): the verified
//! tenant's revision rows, paged in creation order, read from the governed
//! pointer store's owner. The request carries no tenant. The engine writes a
//! row where a revision changes stage ([`record_revision`]); no caller writes
//! one.

use super::SharedState;
use medulla_decision_outcome_types::decision::promotion::{RevisionEvent, RevisionStage};
use medulla_decision_outcome_types::decision::revision_list::{
    cursor_of, key_after_cursor, RevisionProposalListRequest, RevisionProposalPage, RevisionRow,
    REVISION_KEY_PREFIX, REVISION_PROPOSAL_DEFAULT_LIMIT, REVISION_PROPOSAL_PAGE_SCHEMA_VERSION,
};
#[cfg(feature = "redb")]
#[cfg(feature = "decide")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Most revision rows one tenant's relation lists when retiring a superseded
/// champion.
const MAX_REVISIONS: usize = 65_536;

/// Page the verified tenant's revision rows.
#[cfg(feature = "decide")]
pub async fn handle_revision_proposal_list(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: RevisionProposalListRequest,
) -> Response {
    let outcome = async {
        let store = state.write().await.ensure_agent_library()?;
        list_page(&store, verified.tenant(), &request)
    }
    .await;
    match outcome {
        Ok(page) => Response::ok(
            req_id,
            medulla_protocol_types::protocol::ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::RevisionProposalList,
            >(&page),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// A build without the Decide layer refuses by name.
#[cfg(not(feature = "decide"))]
pub async fn handle_revision_proposal_list(
    _state: &SharedState,
    req_id: u64,
    _verified: &VerifiedRequestContext,
    _request: RevisionProposalListRequest,
) -> Response {
    Response::err(req_id, "RevisionProposalList requires the `decide` feature")
}

fn list_page(
    store: &AgentLibraryStore,
    tenant: &str,
    request: &RevisionProposalListRequest,
) -> Result<RevisionProposalPage, String> {
    let after = match request.cursor.as_deref() {
        None => None,
        Some(cursor) => Some(key_after_cursor(cursor).ok_or_else(|| {
            "PROPOSAL_CURSOR_INVALID: the cursor is not a revision page cursor".to_string()
        })?),
    };
    let limit = request
        .limit
        .unwrap_or(REVISION_PROPOSAL_DEFAULT_LIMIT)
        .max(1) as usize;
    let mut stored =
        store.decision_artifacts_page(tenant, REVISION_KEY_PREFIX, after.as_deref(), limit + 1)?;
    let more = stored.len() > limit;
    stored.truncate(limit);
    let rows = stored
        .iter()
        .map(|(_, bytes)| decode_artifact::<RevisionRow>(bytes, "revision row"))
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if more {
        rows.last().map(cursor_of)
    } else {
        None
    };
    Ok(RevisionProposalPage {
        schema_version: REVISION_PROPOSAL_PAGE_SCHEMA_VERSION,
        rows,
        next_cursor,
    })
}

/// Record that `row` exists at its stage. A repeat of the same row is a
/// replay. Where a new champion takes a subject, the subject's previous
/// champion rows retire through `RevisionStage::apply`.
pub(super) fn record_revision(
    store: &AgentLibraryStore,
    tenant: &str,
    row: &RevisionRow,
) -> Result<(), String> {
    if row.stage == RevisionStage::Champion {
        retire_champions(store, tenant, row)?;
    }
    let key = row.key();
    let bytes = encode_artifact(row)?;
    let expected = store.decision_artifact(tenant, &key)?;
    if expected.as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    store.swap_decision_artifact(tenant, &key, expected, bytes)
}

fn retire_champions(
    store: &AgentLibraryStore,
    tenant: &str,
    taking: &RevisionRow,
) -> Result<(), String> {
    for (key, bytes) in
        store.decision_artifacts_with_prefix(tenant, REVISION_KEY_PREFIX, MAX_REVISIONS)?
    {
        let mut held: RevisionRow = decode_artifact(&bytes, "revision row")?;
        let same_subject =
            held.subject_kind == taking.subject_kind && held.subject_ref == taking.subject_ref;
        if !same_subject || held.stage != RevisionStage::Champion || held.id == taking.id {
            continue;
        }
        held.stage = held
            .stage
            .apply(RevisionEvent::Retire)
            .map_err(|refused| format!("REVISION_TRANSITION_REFUSED: {refused:?}"))?;
        store.swap_decision_artifact(tenant, &key, Some(bytes), encode_artifact(&held)?)?;
    }
    Ok(())
}
