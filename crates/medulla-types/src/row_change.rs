//! Committed user-table row changes (MEDULLA-ENGINE-PARITY-R015.1).
//!
//! The table store describes what a committed transaction changed as
//! [`RowChange`]s, in commit order, and hands them to the [`RowChangeSink`] the
//! server installed once the commit is durable. The server's CDC hub is that
//! sink: it appends each change to the table graph's feed. The store owns no
//! feed and the hub owns no table decoder.

use serde::{Deserialize, Serialize};

/// What happened to one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowChangeKind {
    Insert,
    Update,
    Delete,
}

/// One committed row change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowChange {
    pub table: String,
    pub kind: RowChangeKind,
    /// The primary key rendered as text (every cell for a table without one).
    pub key: String,
    /// The row before the change as a MessagePack map; `None` for an insert.
    pub before: Option<Vec<u8>>,
    /// The row after the change as a MessagePack map; `None` for a delete.
    pub after: Option<Vec<u8>>,
}

/// Receives the row changes of one durable commit, in commit order.
pub trait RowChangeSink: Send + Sync + std::fmt::Debug {
    fn publish(&self, graph: &str, changes: Vec<RowChange>);
}
