//! The durable retrieval document-state store (MEDULLA-FEDERATED-QUERY-R081.101.1).
//!
//! One kernel-owned owner file, `doc_state.redb`, of layout
//! `OwnerLayout::RetrievalState`, modelled on the cluster-hierarchy store. Rows are
//! derived and recomputable: a stale or missing row costs a recompute and nothing
//! else. This type is the only caller of the `doc_state` table.

use std::path::Path;
use std::sync::Arc;

use medulla_types::retrieval_round::{DocumentState, DocumentStateError};

/// The owner file name under the persist dir.
pub const DOC_STATE_FILE: &str = "doc_state.redb";
/// The physical authority of [`DOC_STATE_FILE`].
pub const DOC_STATE_PHYSICAL_STORE: &str = "epistemic-graph:retrieval-state";
const DOC_STATE_SCOPE_RESOURCE: &str = "retrieval-state";
const DOC_STATE_SCOPE_INCARNATION: &str = "retrieval-state:v1";

type Durable = medulla_store::sidecar_store::SidecarStore<medulla_storage::RetrievalStateOwner>;

/// Why a store call did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentStateStoreError {
    /// The state is over a bound or does not cover its text (`RETRIEVAL_STATE_INVALID`).
    Invalid(String),
    /// The store itself failed.
    Store(String),
}

impl From<String> for DocumentStateStoreError {
    fn from(error: String) -> Self {
        Self::Store(error)
    }
}

impl From<DocumentStateError> for DocumentStateStoreError {
    fn from(error: DocumentStateError) -> Self {
        match error {
            DocumentStateError::Invalid(detail) => Self::Invalid(detail),
            DocumentStateError::Codec(detail) => Self::Store(detail),
        }
    }
}

/// The retrieval document-state owner file.
pub struct DocumentStateStore {
    durable: Durable,
}

impl std::fmt::Debug for DocumentStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentStateStore").finish_non_exhaustive()
    }
}

impl DocumentStateStore {
    /// Open (or create) `doc_state.redb` under `persist_dir`.
    pub fn open(persist_dir: &str) -> Result<Self, String> {
        let path = Path::new(persist_dir).join(DOC_STATE_FILE);
        let durable = Durable::open(
            &path,
            DOC_STATE_PHYSICAL_STORE,
            DOC_STATE_SCOPE_RESOURCE,
            DOC_STATE_SCOPE_INCARNATION,
            medulla_kernel::store_authority::process_authority(),
        )?;
        Ok(Self { durable })
    }

    /// The state stored under the four-part key, if any.
    pub fn get(
        &self,
        document_id: &str,
        content_digest: &str,
        encoder_revision: &str,
        policy_revision: &str,
    ) -> Result<Option<DocumentState>, String> {
        let key = medulla_types::retrieval_round::state_key(
            document_id,
            content_digest,
            encoder_revision,
            policy_revision,
        );
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::DOC_STATE)?;
        let row = table.get(key.as_str()).map_err(|error| error.to_string())?;
        row.map(|value| DocumentState::decode(value.value()).map_err(|e| format!("{e:?}")))
            .transpose()
    }

    /// Write one state. The value bound refuses `Invalid` before any write.
    /// Idempotent: the same key and content writes the same bytes.
    pub fn put(&self, state: &DocumentState) -> Result<(), DocumentStateStoreError> {
        let key = state.key();
        let bytes = state.encode()?;
        self.durable.maintain("doc_state_put", |owner| {
            let mut table = owner.open_table(medulla_storage::DOC_STATE)?;
            table
                .insert(key.as_str(), bytes.as_slice())
                .map(|_| ())
                .map_err(|error| error.to_string())
        })?;
        Ok(())
    }
}

/// Open (once) the shared state-store handle `ServerState` retains, under the
/// configured persist directory. redb admits one opener per file.
pub fn shared_doc_state_store(
    state: &medulla_server_api::state::ServerState,
) -> Result<Arc<DocumentStateStore>, String> {
    let mut guard = state.doc_state_store.lock();
    if let Some(store) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<DocumentStateStore>().ok())
    {
        return Ok(store);
    }
    let persist_dir = state
        .persist_dir
        .as_deref()
        .ok_or_else(|| "document state requires a configured persist directory".to_string())?;
    let store = Arc::new(DocumentStateStore::open(persist_dir)?);
    *guard = Some(Arc::clone(&store) as medulla_server_api::state::ErasedHandle);
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::retrieval_round::SegmentBoundary;

    fn state(digest: &str) -> DocumentState {
        DocumentState {
            document_id: "d".into(),
            content_digest: digest.into(),
            encoder_revision: "e".into(),
            policy_revision: "p".into(),
            segments: vec![SegmentBoundary { start: 0, end: 3 }],
            projections: vec![vec![1, 2]],
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.101.1
    #[test]
    fn state_survives_reopen_and_a_new_digest_is_a_new_row() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        {
            let store = DocumentStateStore::open(persist).unwrap();
            assert_eq!(store.get("d", "x", "e", "p").unwrap(), None);
            store.put(&state("x")).unwrap();
        }
        let store = DocumentStateStore::open(persist).unwrap();
        assert_eq!(store.get("d", "x", "e", "p").unwrap(), Some(state("x")));
        assert_eq!(store.get("d", "y", "e", "p").unwrap(), None);
        assert_eq!(store.get("d", "x", "e2", "p").unwrap(), None);
    }
}
