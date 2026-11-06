//! Watermark change capture for an attached SQLite file
//! (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.2).
//!
//! A watermark cannot see a delete, so this adapter declares [`CAPTURE_DELETES`] `false`.
//! Per table the declared watermark column is either the integer `rowid` or an integer
//! `updated_at` timestamp; rows whose watermark exceeds the stored position become one
//! [`ChangeEnvelope`] each, with the source position set to the maximum watermark seen.
//! The stored position is a [`ConsumerCheckpoint`], so a restart resumes without a
//! duplicate. The file is read through the lock-free WAL snapshot reader and never
//! written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use medulla_durable::change_envelope::{ChangeEnvelope, ChangeOp};
use medulla_durable::idempotent_consumer::ConsumerCheckpoint;
use medulla_sqlite_format::{Reader, Value};

/// A watermark cannot observe a delete.
pub const CAPTURE_DELETES: bool = false;

/// The watermark column naming the SQLite rowid itself.
pub const ROWID: &str = "rowid";

/// The registered `ServerErrorCode` token of an unusable watermark.
pub const WATERMARK_INVALID: &str = "SQLITE_WATERMARK_INVALID";

/// Why a poll was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureRefusal {
    /// The watermark column is missing from the table or has no index.
    WatermarkInvalid {
        /// The polled table.
        table: String,
        /// The declared watermark column.
        column: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The file could not be read as a stable snapshot.
    Read(String),
}

impl CaptureRefusal {
    /// The wire token of this refusal, when it has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::WatermarkInvalid { .. } => Some(WATERMARK_INVALID),
            Self::Read(_) => None,
        }
    }
}

impl std::fmt::Display for CaptureRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WatermarkInvalid {
                table,
                column,
                reason,
            } => write!(
                f,
                "{WATERMARK_INVALID}: watermark `{column}` of table `{table}` {reason}"
            ),
            Self::Read(detail) => write!(f, "sqlite capture read failed: {detail}"),
        }
    }
}

impl std::error::Error for CaptureRefusal {}

fn wal_path(main: &Path) -> PathBuf {
    let mut wal = main.as_os_str().to_owned();
    wal.push("-wal");
    PathBuf::from(wal)
}

/// Poll `table` of the SQLite file at `path` once. `watermark` is [`ROWID`] or the name of
/// an indexed integer column (`updated_at`). Returns one envelope per row whose watermark
/// is greater than `checkpoint.position`, ordered by watermark then rowid, and advances
/// the checkpoint to the maximum watermark seen. A rowid watermark emits `Insert` (a rowid
/// is only ever new); a column watermark emits `Update` (an insert and an update look the
/// same to it). Rows with a NULL, negative or non-integer watermark are not captured.
pub fn poll_watermark(
    path: &Path,
    table: &str,
    watermark: &str,
    checkpoint: &mut ConsumerCheckpoint,
    busy_timeout: Duration,
) -> Result<Vec<ChangeEnvelope>, CaptureRefusal> {
    let invalid = |reason: &str| CaptureRefusal::WatermarkInvalid {
        table: table.to_string(),
        column: watermark.to_string(),
        reason: reason.to_string(),
    };
    let read = |e: medulla_sqlite_format::Error| CaptureRefusal::Read(e.to_string());
    let reader = Reader::open_with_wal(path, &wal_path(path), busy_timeout).map_err(read)?;
    let slot = if watermark == ROWID {
        None
    } else {
        let columns = reader.table_columns(table).map_err(read)?;
        let slot = columns
            .iter()
            .position(|c| c.name == watermark)
            .ok_or_else(|| invalid("is not a column of the table"))?;
        let indexed = reader.indexed_leading_columns(table).map_err(read)?;
        if !indexed.iter().any(|c| c == watermark) {
            return Err(invalid("has no index"));
        }
        Some(slot)
    };

    let mut changed: Vec<(u64, i64)> = Vec::new();
    for (rowid, row) in reader.scan_table_with_rowids(table).map_err(read)? {
        let mark = match slot {
            None => Some(rowid),
            Some(slot) => match row.get(slot) {
                Some(Value::Integer(i)) => Some(*i),
                _ => None,
            },
        };
        if let Some(mark) = mark.and_then(|m| u64::try_from(m).ok()) {
            if mark > checkpoint.position {
                changed.push((mark, rowid));
            }
        }
    }
    let Some(max) = changed.iter().map(|(mark, _)| *mark).max() else {
        return Ok(Vec::new());
    };
    changed.sort_unstable();
    let op = if slot.is_none() {
        ChangeOp::Insert
    } else {
        ChangeOp::Update
    };
    let envelopes = changed
        .into_iter()
        .map(|(_, rowid)| ChangeEnvelope {
            source_position: max.to_string(),
            table: table.to_string(),
            op,
            key: Some(BTreeMap::from([(ROWID.to_string(), rowid.to_string())])),
            table_is_keyless: false,
        })
        .collect();
    checkpoint.apply_if_newer(max);
    Ok(envelopes)
}
