//! The retrieval document-state owner table (MEDULLA-FEDERATED-QUERY-R081.101.1).
//!
//! One row per `(document_id, content_digest, encoder_revision, policy_revision)`,
//! the key being `medulla_types::retrieval_round::state_key`, valued with the
//! MessagePack `DocumentState`. Rows are derived and recomputable. `medulla-storage`
//! declares the table; the store in `medulla-persistence` only names it.

use redb::TableDefinition;

/// `state_key -> msgpack(DocumentState)`.
pub const DOC_STATE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("doc_state");
