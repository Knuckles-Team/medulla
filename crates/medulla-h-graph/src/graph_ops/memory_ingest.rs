//! A committed chat-log thread becomes engine memory (MEDULLA-DECISION-ENGINE-R617.3).
//!
//! The `SourceIngest` route calls [`remember_thread`] after it commits a transcript thread. The
//! thread projects to one memory record per turn (`ConversationRecord::to_memory_records`,
//! MEDULLA-DECISION-ENGINE-R617.2), and each record is written through the served memory surface
//! (`MemoryRemember`'s handler), so it is admitted, quota-counted and replicated like every other
//! memory write. Turns are sealed into episodes by `knob episodes.gap`
//! (MEDULLA-DECISION-ENGINE-R155.3): a quiet period longer than the knob closes an episode, and
//! the last run stays open, because a later turn may still extend it. The harness reads the
//! memory back only through `MemoryRecall`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_memory::memory::episodes::split_by_gap;
use medulla_decision_types::decision::knob_families::episodes::GAP;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_protocol_types::protocol::Method;
use medulla_server_mutation::mutation::MutationCtx;
use medulla_work_types::memory::{
    ConversationRecord, EpisodeSpec, MemoryRememberRequest, MemoryTier,
};

/// What one thread wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RememberedThread {
    /// Records submitted (a record already stored is submitted again and writes nothing).
    pub records: usize,
    /// Episodes sealed or found sealed.
    pub episodes: usize,
}

/// `knob episodes.gap` in milliseconds.
fn gap_ms() -> i64 {
    match GAP.default {
        KnobValue::DurationMs(ms) => i64::try_from(ms).unwrap_or(i64::MAX),
        _ => i64::MAX,
    }
}

/// Embed `texts` off the reactor; the configured embedder, or its hash fallback.
async fn embed(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env().embed(&refs)
    })
    .await
    .map_err(|error| format!("INTERNAL: embedding task failed: {error}"))?
}

/// The remembers one thread writes, in write order (MEDULLA-DECISION-ENGINE-R617.3). `embeddings`
/// holds one vector per message, in message order. A closed run of episodic turns seals into one
/// episode, carried by the run's last record.
pub(super) fn thread_requests(
    conversation: &ConversationRecord,
    embeddings: Vec<Vec<f32>>,
    gap_ms: i64,
) -> Result<Vec<MemoryRememberRequest>, String> {
    let records = conversation
        .to_memory_records()
        .map_err(|error| format!("PARAMETER_INVALID: {error}"))?;
    if embeddings.len() != records.len() {
        return Err("INTERNAL: the embedder answered a different number of vectors".to_string());
    }
    // Write in time order, so a sealed episode's earlier records are stored before it.
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by_key(|&index| (records[index].timestamp_ms, index));
    // Episodes group the episodic turns that carry a timestamp.
    let episodic: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&index| records[index].tier == MemoryTier::Episodic && records[index].timestamp_ms > 0)
        .collect();
    let stamps: Vec<i64> = episodic.iter().map(|&index| records[index].timestamp_ms).collect();
    let runs = split_by_gap(&stamps, gap_ms);
    let closed = runs.len().saturating_sub(1);
    let mut specs: BTreeMap<usize, EpisodeSpec> = BTreeMap::new();
    for run in runs.into_iter().take(closed) {
        let members = &episodic[run];
        let (Some(&first), Some(&last)) = (members.first(), members.last()) else {
            continue;
        };
        specs.insert(
            last,
            EpisodeSpec {
                episode_id: super::memory_surface::episode_id_for(&records[first])?,
                session_id: records[last].session_id.clone(),
                preceding: members[..members.len() - 1]
                    .iter()
                    .map(|&index| records[index].memory_id.clone())
                    .collect(),
            },
        );
    }
    let mut embeddings: Vec<Option<Vec<f32>>> = embeddings.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .map(|index| MemoryRememberRequest {
            record: records[index].clone(),
            embedding: embeddings[index].take().unwrap_or_default(),
            episode: specs.remove(&index),
        })
        .collect())
}

/// Write the memory of one committed thread under the caller's tenant. Re-writing the same
/// thread writes nothing new: a stored record and a sealed episode are found again.
pub async fn remember_thread(
    ctx: &MutationCtx<'_>,
    conversation: &ConversationRecord,
) -> Result<RememberedThread, String> {
    // A thread of another tenant answers like one that does not exist.
    if conversation.tenant.as_str() != ctx.tenant_scope {
        return Err(super::memory_surface::absent_record(conversation.conversation_id.as_str()));
    }
    if conversation.messages.is_empty() {
        return Ok(RememberedThread { records: 0, episodes: 0 });
    }
    let embeddings = embed(conversation.messages.iter().map(|message| message.text.clone()).collect()).await?;
    let requests = thread_requests(conversation, embeddings, gap_ms())?;
    let summary = RememberedThread {
        records: requests.len(),
        episodes: requests.iter().filter(|request| request.episode.is_some()).count(),
    };
    for request in requests {
        let method = Method::MemoryRemember { request: Box::new(request) };
        let response = super::memory_surface::handle_remember(ctx, ctx.tenant_scope, &method, &BTreeSet::new())
            .await
            .ok_or_else(|| "INTERNAL: the memory surface did not take the write".to_string())?;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
    }
    Ok(summary)
}
