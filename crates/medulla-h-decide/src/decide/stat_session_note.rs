//! Session-note capture (MEDULLA-DECISION-ENGINE-R150.3, R150.4).
//!
//! A session note is stored only after a successful run: the receipt of its
//! `run_id` (R100.102.5) must exist and record success. The note is embedded
//! with the engine's configured embedder and staged as a memory record plus
//! its sidecar row, which the served `DecisionLog` commits through the durable
//! mutation path. `MemoryRecall` reads it back and flags it with
//! `annotate_notes` (R150.2).

use medulla_protocol_types::protocol::Method;
use medulla_work_types::memory::{
    SessionNoteBody, SessionNoteStored, NOTE_RUN_NOT_SUCCESSFUL, NOTE_USER_MISSING,
};

use super::stat_executor::ExecutionContext;
use super::stat_generation_cycle::Reembedder;
use super::stat_log::LogReader;
use super::stat_run_credit::stored_receipt;
use super::stat_vectors::GraphVectors;

/// Validate and stage one note. Every refusal is decided before anything is
/// staged, so a refused note writes nothing.
pub(super) fn record_note(
    ctx: &ExecutionContext,
    reader: &LogReader,
    tenant_scope: &str,
    graph: &GraphVectors,
    embedder: &dyn Reembedder,
    note: SessionNoteBody,
) -> Result<(SessionNoteStored, Vec<Method>), String> {
    if note.user_id.trim().is_empty() {
        return Err(format!("{}: user_id is empty", NOTE_USER_MISSING.as_str()));
    }
    let receipt = stored_receipt(ctx.store, ctx.tenant_id, &note.run_id)?
        .filter(|receipt| receipt.success)
        .ok_or_else(|| {
            format!(
                "{}: run {} has no successful receipt",
                NOTE_RUN_NOT_SUCCESSFUL.as_str(),
                note.run_id
            )
        })?;
    let embedding = embedder
        .embed(std::slice::from_ref(&note.content))?
        .into_iter()
        .next()
        .ok_or_else(|| "the embedder returned no vector for the note".to_string())?;
    let now_ms = i64::try_from(ctx.now_ms).unwrap_or(i64::MAX);
    let (methods, stored, record_digest) =
        medulla_h_graph_gateway::graph_ops::memory_surface::stage_session_note(
            graph.core(),
            tenant_scope,
            &reader.principal,
            &note,
            embedding,
            now_ms,
        )?;
    Ok((
        SessionNoteStored {
            note_id: note.note_id,
            run_receipt_digest: receipt.receipt_digest,
            record_digest,
            stored,
        },
        methods,
    ))
}
