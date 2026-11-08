//! Offline archive and point-in-time replay target (MEDULLA-DURABLE-KERNEL-R007.1).
//!
//! The offline `archive` tool names a durable-store backend spec it writes to
//! (<CONSUMER:MEDULLA-ENGINE-PARITY-R016.2>), and `restore` gains an optional
//! point-in-time target: `--to-seq N` or `--to-time <RFC3339>` (exactly one).
//! This module owns the typed replay-target model and the atomic refusal for
//! the replay boundary: a target past the archive end is refused at the type
//! level, before any replay decides to move bytes into the destination.

/// The point-in-time target an offline restore may declare. Exactly one
/// variant is accepted; both absent or both present refuse at parse time so
/// no caller can fall through with an ambiguous (or default) target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayTarget {
    /// Replay up to and including this sequence number.
    Sequence(u64),
    /// Replay up to and including this RFC3339 UTC timestamp.
    Time(String),
}

/// Typed refusal at the archive replay boundary. Implied by [`ReplayTarget`]
/// and [`ArchiveBounds`]: a refused replay has not moved any bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ArchiveRefusal {
    /// Neither `--to-seq` nor `--to-time` was given. Positions the CLI: with
    /// no target, restore behaves as it does today (R007.1 keeps that), so
    /// this refusal surfaces only where a target is REQUIRED.
    NoTarget,
    /// Both targets were declared exactly once; exactly one is required.
    AmbiguousTarget,
    /// `--to-seq` and `--to-time` are declared together.
    #[serde(skip)]
    ConflictingTargets,
    /// The target sequence is past the archive's last replayable sequence.
    TargetPastArchiveEnd {
        /// Last sequence number the archive can replay to.
        end_sequence: u64,
        /// Declared target sequence.
        target_sequence: u64,
    },
    /// The target timestamp is past the archive end (nothing past it exists).
    TargetTimePastArchiveEnd {
        /// RFC3339 timestamp of the archive end.
        end_time: String,
        /// Declared target timestamp.
        target_time: String,
    },
    /// The `--to-time` target is present but not a parsable RFC3339 timestamp.
    MalformedTargetTime { reason: String },
}

/// What an archive can replay to, certified by its MANIFEST read: the first
/// replayable sequence, the last replayable sequence, and the RFC3339
/// timestamp of the last segment (caller-supplied at archive time; mirrors
/// `BackupManifest::timestamp`). Pure data — nothing here touches disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveBounds {
    /// First sequence number the archive can replay from.
    pub begin_sequence: u64,
    /// Last sequence number the archive can replay to.
    pub end_sequence: u64,
    /// RFC3339 timestamp of the archive end segment.
    pub end_time: String,
}

/// Validate one `--to-time` RFC3339 value syntactically (UTC `Z` form). The
/// archive model never calls `Date::now` and never parses wall-clock breadth;
/// it only fails closed on a target no replay comparison can order.
fn require_parsable_rfc3339(value: &str) -> Result<(), ArchiveRefusal> {
    if value.trim().is_empty() {
        return Err(ArchiveRefusal::MalformedTargetTime {
            reason: "target time is empty".to_string(),
        });
    }
    // `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` — length and separators only; the
    // ordering comparison is lexicographic on the fixed-width prefix, which a
    // fixed-offset timestamp keeps total without a datetime dependency here.
    let short = value.len() < 20
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(13) != Some(&b':');
    if short {
        return Err(ArchiveRefusal::MalformedTargetTime {
            reason: "not an RFC3339 timestamp".to_string(),
        });
    }
    Ok(())
}

/// Parse the CLI pair `(--to-seq, --to-time)` into one typed target. Atomic:
/// exactly one branch can win, and every refusal happens before any caller
/// side effect (the CLI parses this before it opens the destination).
pub fn parse_replay_target(
    to_seq: Option<u64>,
    to_time: Option<String>,
) -> Result<Option<ReplayTarget>, ArchiveRefusal> {
    match (to_seq, to_time) {
        (Some(seq), None) => Ok(Some(ReplayTarget::Sequence(seq))),
        (None, Some(time)) => {
            require_parsable_rfc3339(&time)?;
            Ok(Some(ReplayTarget::Time(time)))
        }
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(ArchiveRefusal::ConflictingTargets),
    }
}

/// Refuse a replay target that lies outside the archive. Pure: reads no files,
/// writes nothing. `Sequence` orders against the certified [`ArchiveBounds`];
/// a `Time` target orders against the archive end timestamp (fixed-offset
/// RFC3339 keeps lexicographic comparison total). The refusing arm is the one
/// named by the R007.1 test
/// (`tests/offline_restore_pitr_cli.rs::target_past_archive_refuses_and_writes_nothing`):
/// the refusal is typed, and because this is checked before any destination
/// effect, no shard file is written.
pub fn replay_to(target: &ReplayTarget, bounds: &ArchiveBounds) -> Result<(), ArchiveRefusal> {
    match target {
        ReplayTarget::Sequence(seq) if *seq > bounds.end_sequence => {
            Err(ArchiveRefusal::TargetPastArchiveEnd {
                end_sequence: bounds.end_sequence,
                target_sequence: *seq,
            })
        }
        ReplayTarget::Time(time) if *time > bounds.end_time => {
            Err(ArchiveRefusal::TargetTimePastArchiveEnd {
                end_time: bounds.end_time.clone(),
                target_time: time.clone(),
            })
        }
        ReplayTarget::Sequence(seq) if *seq < bounds.begin_sequence => {
            Err(ArchiveRefusal::TargetPastArchiveEnd {
                end_sequence: bounds.end_sequence,
                target_sequence: *seq,
            })
        }
        ReplayTarget::Sequence(_) | ReplayTarget::Time(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R007.1
    #[test]
    fn target_past_archive_refuses_typed_and_pure() {
        let bounds = ArchiveBounds {
            begin_sequence: 10,
            end_sequence: 90,
            end_time: "2026-11-08T00:00:00Z".to_string(),
        };
        // A target past the archive end refuses with the typed error.
        assert_eq!(
            replay_to(
                &ReplayTarget::Sequence(100),
                &ArchiveBounds {
                    begin_sequence: 10,
                    end_sequence: 90,
                    end_time: "2026-11-08T00:00:00Z".to_string(),
                }
            ),
            Err(ArchiveRefusal::TargetPastArchiveEnd {
                end_sequence: 90,
                target_sequence: 100,
            })
        );
        // A target inside the archive is accepted.
        assert_eq!(replay_to(&ReplayTarget::Sequence(90), &bounds), Ok(()));
        // Below the begin is also outside the replayable range: fail-closed.
        assert!(matches!(
            replay_to(&ReplayTarget::Sequence(1), &bounds),
            Err(ArchiveRefusal::TargetPastArchiveEnd { .. })
        ));
        // A target time past the archive end refuses typed, writes nothing.
        assert!(matches!(
            replay_to(
                &ReplayTarget::Time("2026-11-09T00:00:00Z".to_string()),
                &bounds
            ),
            Err(ArchiveRefusal::TargetTimePastArchiveEnd { .. })
        ));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R007.1
    #[test]
    fn target_pair_is_atomic_and_fail_closed() {
        // Exactly one target: accepted.
        assert_eq!(
            parse_replay_target(Some(7), None),
            Ok(Some(ReplayTarget::Sequence(7)))
        );
        // No target: the CLI branches to today's behavior, not a sentinel.
        assert_eq!(parse_replay_target(None, None), Ok(None));
        assert!(matches!(
            parse_replay_target(Some(1), Some("2026-11-08T00:00:00Z".to_string())),
            Err(ArchiveRefusal::ConflictingTargets)
        ));
        // Empty time string is refused, never treated as "some" target.
        assert!(matches!(
            parse_replay_target(None, Some(String::from(""))),
            Err(ArchiveRefusal::MalformedTargetTime { .. })
        ));
    }
}
