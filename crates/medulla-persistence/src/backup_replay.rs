//! Point-in-time restore from archive segments (`MEDULLA-ENGINE-PARITY-R016.2`).
//!
//! [`crate::backup::restore_bundle`] restores a bundle verbatim; when the
//! archive is configured the offline restore then calls [`replay_archive`] to
//! apply the archived batch records past the bundle, up to a [`ReplayTarget`].
//! Every record goes through the live commit path
//! ([`PersistenceBackend::commit_mutation_batch`]), so admission and
//! idempotency run as they do for a served write. [`archive_bounds`] certifies
//! what the archive can replay to, and the target is checked against it before
//! any destination effect.

use std::collections::BTreeMap;
use std::path::Path;

use medulla_mutation_types::mutation_scope::MutationScope;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_storage::object::archive::{ArchiveRefusal as ObjectRefusal, ObjectArchive};
use medulla_storage::{
    decode_batch_record, ArchiveBounds, ArchiveRefusal, ArchiveSegment, ReplayTarget,
};

use crate::backup::rfc3339_from_timestamp;
use crate::redb_backend::RedbBackend;

/// What one archive replay did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayReport {
    /// Segments read from the archive and walked.
    pub segments_replayed: u64,
    /// Records the live commit path accepted as new.
    pub records_applied: u64,
    /// Scope key to the last applied sequence of that scope; scopes with no
    /// applied record are absent.
    pub final_sequence: BTreeMap<String, u64>,
}

/// The typed refusal as the JSON text the offline CLI reports.
fn unavailable(detail: String) -> String {
    let refusal = ArchiveRefusal::RecoveryTargetUnavailable { detail };
    serde_json::to_string(&refusal).unwrap_or_else(|_| format!("{refusal:?}"))
}

fn segment_unavailable(scope: &str, first: u64, last: u64, refusal: &ObjectRefusal) -> String {
    unavailable(format!(
        "{scope}/{first}-{last}: {}: {refusal:?}",
        refusal.wire_code()
    ))
}

fn archive_failure(refusal: &ObjectRefusal) -> String {
    unavailable(format!("{}: {refusal:?}", refusal.wire_code()))
}

fn decode_segment(
    scope: &str,
    first: u64,
    last: u64,
    bytes: &[u8],
) -> Result<ArchiveSegment, String> {
    rmp_serde::from_slice(bytes)
        .map_err(|error| unavailable(format!("{scope}/{first}-{last}: segment decode: {error}")))
}

/// Unix milliseconds of an RFC3339 timestamp
/// (`YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)`).
fn rfc3339_to_ms(text: &str) -> Result<u64, String> {
    let bad = || format!("not an RFC3339 timestamp: {text}");
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.is_ascii() {
        return Err(bad());
    }
    let num = |from: usize, to: usize| -> Result<i64, String> {
        text[from..to].parse::<i64>().map_err(|_| bad())
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut index = 19;
    let mut millis = 0i64;
    if bytes[index] == b'.' {
        let start = index + 1;
        index = start;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let digits = &text[start..index];
        let padded: String = digits.chars().chain("000".chars()).take(3).collect();
        millis = padded.parse::<i64>().map_err(|_| bad())?;
    }
    let offset_minutes = match bytes.get(index).copied() {
        Some(b'Z' | b'z') if index + 1 == bytes.len() => 0,
        Some(sign @ (b'+' | b'-')) if bytes.len() == index + 6 => {
            let hours = num(index + 1, index + 3)?;
            let minutes = num(index + 4, index + 6)?;
            let total = hours * 60 + minutes;
            if sign == b'+' {
                total
            } else {
                -total
            }
        }
        _ => return Err(bad()),
    };
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds * 1_000 + millis).map_err(|_| bad())
}

/// What the archive can replay to: the first replayable sequence (0), the
/// largest `last - 1` over every scope, and the RFC3339 form of the largest
/// `committed_at_ms` in the last segment of any scope. Reads the manifests and
/// the last segment of each scope only.
pub async fn archive_bounds(archive: &ObjectArchive, tenant: &str) -> Result<ArchiveBounds, String> {
    let mut end_sequence = 0u64;
    let mut end_ms = 0u64;
    for scope in archive
        .list_scopes(tenant)
        .await
        .map_err(|refusal| archive_failure(&refusal))?
    {
        let entries = archive
            .segments(tenant, &scope)
            .await
            .map_err(|refusal| archive_failure(&refusal))?;
        let Some(tail) = entries.iter().max_by_key(|entry| entry.last) else {
            continue;
        };
        end_sequence = end_sequence.max(tail.last.saturating_sub(1));
        let bytes = archive
            .get_segment(tenant, &scope, tail.first, tail.last)
            .await
            .map_err(|refusal| segment_unavailable(&scope, tail.first, tail.last, &refusal))?;
        let segment = decode_segment(&scope, tail.first, tail.last, &bytes)?;
        for stored in &segment.records {
            let record = decode_batch_record(stored).map_err(|error| {
                unavailable(format!("{scope}/{}-{}: {error}", tail.first, tail.last))
            })?;
            end_ms = end_ms.max(record.committed_at_ms);
        }
    }
    Ok(ArchiveBounds {
        begin_sequence: 0,
        end_sequence,
        end_time: rfc3339_from_timestamp(end_ms / 1_000),
    })
}

/// Apply the archived records of every scope, in order, to the restored
/// directory `persist_dir` up to `target`. Records already inside the restored
/// ledger are skipped, a scope stops at its first record past the target, and
/// each kept record is committed through the live commit path.
pub async fn replay_archive(
    persist_dir: &Path,
    shards: usize,
    archive: &ObjectArchive,
    tenant: &str,
    target: &ReplayTarget,
) -> Result<ReplayReport, String> {
    let target_ms = match target {
        ReplayTarget::Time(time) => Some(rfc3339_to_ms(time)?),
        ReplayTarget::Sequence(_) => None,
    };
    let backend = RedbBackend::open_with_shards(
        persist_dir.to_string_lossy().into_owned(),
        1024,
        shards,
    )?;
    let restored = backend.last_committed_sequences()?;
    let mut report = ReplayReport::default();
    let scopes = archive
        .list_scopes(tenant)
        .await
        .map_err(|refusal| archive_failure(&refusal))?;
    for scope in scopes {
        let mut entries = archive
            .segments(tenant, &scope)
            .await
            .map_err(|refusal| archive_failure(&refusal))?;
        entries.sort_by_key(|entry| entry.first);
        let restored_end = restored.get(&scope).copied();
        let mut finished = false;
        for entry in entries {
            if finished {
                break;
            }
            let bytes = archive
                .get_segment(tenant, &scope, entry.first, entry.last)
                .await
                .map_err(|refusal| segment_unavailable(&scope, entry.first, entry.last, &refusal))?;
            let segment = decode_segment(&scope, entry.first, entry.last, &bytes)?;
            report.segments_replayed += 1;
            for stored in &segment.records {
                let record = decode_batch_record(stored).map_err(|error| {
                    unavailable(format!("{scope}/{}-{}: {error}", entry.first, entry.last))
                })?;
                let Some(seq) = record.committed_version.target() else {
                    continue;
                };
                if restored_end.is_some_and(|end| seq < end) {
                    continue;
                }
                let past = match (target, target_ms) {
                    (ReplayTarget::Sequence(n), _) => seq > *n,
                    (ReplayTarget::Time(_), Some(ms)) => record.committed_at_ms > ms,
                    (ReplayTarget::Time(_), None) => false,
                };
                if past {
                    finished = true;
                    break;
                }
                let MutationScope::Graph { graph } = record.identity.scope() else {
                    return Err(unavailable(format!(
                        "{scope}/{seq}: the scope kind has no owner-store replay path"
                    )));
                };
                let commit = backend
                    .commit_mutation_batch(
                        graph.as_str(),
                        &record.batch,
                        record.result_msgpack.as_deref(),
                        record.committed_at_ms,
                    )
                    .await?;
                if commit.replayed {
                    continue;
                }
                report.records_applied += 1;
                report.final_sequence.insert(scope.clone(), seq);
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::rfc3339_to_ms;

    // spec: MEDULLA-ENGINE-PARITY-R016.2
    #[test]
    fn rfc3339_parses_to_unix_milliseconds() {
        assert_eq!(rfc3339_to_ms("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(
            rfc3339_to_ms("2026-11-08T00:00:01.250Z").unwrap(),
            1_794_096_001_250
        );
        assert_eq!(
            rfc3339_to_ms("2026-11-08T02:00:00+02:00").unwrap(),
            rfc3339_to_ms("2026-11-08T00:00:00Z").unwrap()
        );
        assert!(rfc3339_to_ms("not a time").is_err());
    }
}
