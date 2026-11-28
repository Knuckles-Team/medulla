//! The decision-log reader and its visibility rule, split out so the executor and the learning jobs read the log without the serving path.

use super::stat_executor::ExecutionContext;
use super::stat_retention::Retention;
use super::stat_support::refusal;
use super::SharedState;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::record_key;
use medulla_server_api::authority_context::VerifiedRequestContext;
use {
    medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    medulla_decision_types::decision::statistical::log::RecordVisibility,
};

/// Most log rows one read walks before refusing as unbounded.
pub const MAX_LOG_ROWS: usize = 100_000;

/// Who is reading or writing the log.
pub struct LogReader {
    pub tenant_id: String,
    pub principal: String,
    /// The verified request context's role names: a role-named evaluator's
    /// grant admits a holder (EH-395). Empty where no evaluation is served.
    pub roles: Vec<String>,
    /// How compacted inputs are restored when a read needs them.
    pub retention: Retention,
}

impl LogReader {
    /// A reader that needs no compacted inputs (aggregates, visibility).
    pub fn of(verified: &VerifiedRequestContext) -> Self {
        Self {
            tenant_id: verified.tenant().to_string(),
            principal: verified.principal_persistence_id(),
            roles: verified.roles().to_vec(),
            retention: Retention::none(),
        }
    }

    /// A reader that restores compacted inputs from the served Blob CAS.
    pub async fn served(state: &SharedState, verified: &VerifiedRequestContext) -> Self {
        let retention = Retention::of(&*state.read().await, verified);
        Self {
            retention,
            ..Self::of(verified)
        }
    }

    pub(super) fn sees(&self, entry: &DecisionLogEntry) -> bool {
        match &entry.visibility {
            RecordVisibility::Tenant => true,
            RecordVisibility::Principal { principal } => *principal == self.principal,
        }
    }
}

pub fn visible_entries(
    store: &AgentLibraryStore,
    reader: &LogReader,
) -> Result<Vec<DecisionLogEntry>, String> {
    store
        .decision_artifacts_with_prefix(&reader.tenant_id, "record:", MAX_LOG_ROWS)?
        .into_iter()
        .map(|(_, bytes)| decode_artifact::<DecisionLogEntry>(&bytes, "decision log entry"))
        .filter(|entry| !matches!(entry, Ok(e) if !reader.sees(e)))
        .collect()
}

/// One earlier decision that shares the current decision's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedTrajectory {
    pub record_id: String,
    /// The option that record executed.
    pub option_id: String,
}

pub fn visible_entry(
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
) -> Result<Option<DecisionLogEntry>, String> {
    let Some(bytes) = store.decision_artifact(&reader.tenant_id, &record_key(record_id))? else {
        return Ok(None);
    };
    let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
    Ok(reader.sees(&entry).then_some(entry))
}

/// Store `fresh` under `key` exactly once. A re-send with the same content
/// from the same producer answers the stored row (idempotent replay);
/// anything else under that key is a conflict, never an overwrite.
pub fn store_once<T: serde::Serialize + serde::de::DeserializeOwned>(
    ctx: &ExecutionContext,
    key: String,
    noun: &str,
    fresh: T,
    same: impl Fn(&T, &T) -> bool,
) -> Result<T, String> {
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let existing: T = decode_artifact(&bytes, noun)?;
        if same(&existing, &fresh) {
            return Ok(existing);
        }
        return Err(refusal(
            StatisticalErrorCode::IdempotencyConflict,
            format!("the {noun} id is recorded with other content"),
        ));
    }
    ctx.store
        .put_decision_artifacts(ctx.tenant_id, &[(key, encode_artifact(&fresh)?)])?;
    Ok(fresh)
}
