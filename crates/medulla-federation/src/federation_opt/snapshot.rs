//! One read-only snapshot per source per query (MEDULLA-UNIFIED-DATA-PLANE-R038.3.2).
//!
//! The first fragment of a transactional source opens the snapshot through
//! [`RemoteFetch::open_snapshot`](super::remote::RemoteFetch::open_snapshot); the session keeps the
//! handle by source fingerprint and every later fragment of that source reads through it. The
//! handle drops at query end, which rolls the transaction back.

use std::sync::Arc;

use serde::Serialize;

use medulla_exec::rowset::RowSet;
use medulla_types::metadata_acceleration::SnapshotSemantics;

/// The bounds a snapshot is opened under. Both come from the server knobs
/// `federation.replica_lag_bound_ms` and `federation.statement_timeout_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotBound {
    /// A declared replica serves only when its replay lag is at most this many milliseconds.
    pub replica_lag_bound_ms: u64,
    /// Each statement stops at this many milliseconds.
    pub statement_timeout_ms: u64,
}

impl Default for SnapshotBound {
    fn default() -> Self {
        Self {
            replica_lag_bound_ms: 5_000,
            statement_timeout_ms: 30_000,
        }
    }
}

/// A connection held open for the query's snapshot. Dropping the last reference rolls the
/// transaction back.
pub trait SnapshotConn: Send + Sync {
    /// Run one read statement inside the held transaction.
    fn run(&self, statement: &str) -> Result<RowSet, String>;
}

/// One opened snapshot, kept in the session by source fingerprint.
#[derive(Clone)]
pub struct SnapshotHandle {
    pub semantics: SnapshotSemantics,
    pub(crate) conn: Option<Arc<dyn SnapshotConn>>,
}

impl std::fmt::Debug for SnapshotHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotHandle")
            .field("semantics", &self.semantics)
            .field("held", &self.conn.is_some())
            .finish()
    }
}

/// How a query's fragments of one source relate in time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Consistency {
    /// Every fragment of a source reads one snapshot (or the query refuses).
    #[default]
    Snapshot,
    /// The caller accepted per-fragment reads (`consistency: per_fragment`).
    PerFragment,
}

impl Consistency {
    /// The wire word of the request field `consistency`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "snapshot" => Some(Self::Snapshot),
            "per_fragment" => Some(Self::PerFragment),
            _ => None,
        }
    }

    /// The word printed in a trace.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::PerFragment => "per_fragment",
        }
    }
}
