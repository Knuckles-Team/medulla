//! The archiver: copy committed ledger batch records into archive segments
//! (`MEDULLA-ENGINE-PARITY-R016.1`).
//!
//! One pass reads the `ledger_batches` table of one owner file, groups the
//! committed records by scope (`identity_key`), and writes each scope's records
//! that lie at or after its archived position as ordered, size-bounded segments
//! through [`ObjectArchive::put_segment`]. The archived position is derived from
//! the archive manifest ([`ObjectArchive::archived_end`]) and never stored, and
//! the manifest append is the last step of `put_segment`, so a pass interrupted
//! at any point resumes from `archived_end` with no gap and no duplicate. The
//! point-in-time target type stays [`super::archive::ReplayTarget`].

use crate::codec::decode_batch_record;
use crate::object::archive::{ArchiveRefusal, ObjectArchive};
use crate::tables::BATCHES;
use crate::StorageKernel;
use medulla_protocol_types::mutation_batch::MutationBatchStatus;
use redb::ReadableTable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fixed overhead charged to one encoded segment on top of its scope key and
/// records (map, field names and array header), a safe upper bound.
const SEGMENT_OVERHEAD_BYTES: u64 = 64;
/// Overhead charged per record (msgpack binary header), a safe upper bound.
const RECORD_OVERHEAD_BYTES: u64 = 5;

/// The three `archive.*` knobs of one archiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePolicy {
    /// Upper bound on one encoded segment; a record is never split, so one
    /// record larger than this forms its own segment.
    pub segment_bytes: u64,
    /// Segment retention in seconds (`MEDULLA-ENGINE-PARITY-R016.4`).
    pub retain_s: u64,
    /// Seconds between archive passes.
    pub interval_s: u64,
}

/// One archive segment: the stored ledger bytes of consecutive committed
/// records of one scope, exactly as the `ledger_batches` table holds them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveSegment {
    pub scope_key: String,
    pub records: Vec<serde_bytes::ByteBuf>,
}

/// What one archive pass wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchivePassReport {
    pub segments_written: u64,
    pub records_archived: u64,
}

fn refusal_text(refusal: &ArchiveRefusal) -> String {
    format!("{}: {refusal:?}", refusal.wire_code())
}

type ScopeRecords = BTreeMap<String, Vec<(u64, Vec<u8>)>>;

/// Committed records of the store grouped by scope, each `(seq, stored bytes)`
/// sorted by `seq`.
fn committed_by_scope(kernel: &StorageKernel) -> Result<ScopeRecords, String> {
    let rtx = kernel.store().begin_read()?;
    let table = rtx.open_table(BATCHES).map_err(|error| error.to_string())?;
    let mut scopes: ScopeRecords = BTreeMap::new();
    for row in table.iter().map_err(|error| error.to_string())? {
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (identity_key, _batch_id) = key.value();
        let bytes = value.value();
        let record = decode_batch_record(bytes)?;
        if record.status != MutationBatchStatus::Committed {
            continue;
        }
        let Some(seq) = record.committed_version.target() else {
            continue;
        };
        scopes
            .entry(identity_key.to_string())
            .or_default()
            .push((seq, bytes.to_vec()));
    }
    for records in scopes.values_mut() {
        records.sort_by_key(|(seq, _)| *seq);
    }
    Ok(scopes)
}

/// The last committed sequence of every scope of the store, keyed by scope key.
/// A scope with no committed record is absent. Restore replay reads it to skip
/// the records a restored bundle already holds (`MEDULLA-ENGINE-PARITY-R016.2`).
pub fn last_committed_sequences(kernel: &StorageKernel) -> Result<BTreeMap<String, u64>, String> {
    Ok(committed_by_scope(kernel)?
        .into_iter()
        .filter_map(|(scope, records)| records.last().map(|(seq, _)| (scope, *seq)))
        .collect())
}

/// Pack records, in order, into runs whose encoded segment stays within
/// `segment_bytes`; a record larger than the bound forms its own run.
fn pack(
    scope_key: &str,
    pending: &[(u64, Vec<u8>)],
    segment_bytes: u64,
) -> Vec<std::ops::Range<usize>> {
    let base = SEGMENT_OVERHEAD_BYTES + scope_key.len() as u64;
    let mut runs = Vec::new();
    let mut start = 0usize;
    let mut size = base;
    for (index, (_, bytes)) in pending.iter().enumerate() {
        let cost = bytes.len() as u64 + RECORD_OVERHEAD_BYTES;
        if index > start && size + cost > segment_bytes {
            runs.push(start..index);
            start = index;
            size = base;
        }
        size += cost;
    }
    if start < pending.len() {
        runs.push(start..pending.len());
    }
    runs
}

/// Run one archive pass over `kernel`'s store. Returns the segments written and
/// the records they hold. A refusal of `put_segment` stops the pass for that
/// scope; the first refusal is returned as `"<wire_code>: <detail>"` after the
/// other scopes have been archived.
pub async fn archive_store_pass(
    kernel: &StorageKernel,
    archive: &ObjectArchive,
    tenant: &str,
    policy: &ArchivePolicy,
) -> Result<ArchivePassReport, String> {
    let scopes = committed_by_scope(kernel)?;
    let mut report = ArchivePassReport::default();
    let mut first_refusal: Option<String> = None;
    for (scope_key, records) in scopes {
        let archived_end = match archive.archived_end(tenant, &scope_key).await {
            Ok(end) => end,
            Err(refusal) => {
                first_refusal.get_or_insert(refusal_text(&refusal));
                continue;
            }
        };
        let pending: Vec<(u64, Vec<u8>)> = records
            .into_iter()
            .filter(|(seq, _)| *seq >= archived_end)
            .collect();
        for run in pack(&scope_key, &pending, policy.segment_bytes) {
            let slice = &pending[run];
            let segment = ArchiveSegment {
                scope_key: scope_key.clone(),
                records: slice
                    .iter()
                    .map(|(_, bytes)| serde_bytes::ByteBuf::from(bytes.clone()))
                    .collect(),
            };
            let encoded = rmp_serde::to_vec_named(&segment).map_err(|error| error.to_string())?;
            let first_seq = slice[0].0;
            let last_seq = slice[slice.len() - 1].0;
            match archive
                .put_segment(tenant, &scope_key, first_seq, last_seq + 1, &encoded)
                .await
            {
                Ok(_) => {
                    report.segments_written += 1;
                    report.records_archived += slice.len() as u64;
                }
                Err(refusal) => {
                    first_refusal.get_or_insert(refusal_text(&refusal));
                    break;
                }
            }
        }
    }
    match first_refusal {
        Some(refusal) => Err(refusal),
        None => Ok(report),
    }
}
