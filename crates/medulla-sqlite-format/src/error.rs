//! Typed error enum for the SQLite file-format layer: never `unwrap` on untrusted bytes.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// The bytes are not a well-formed SQLite database (bad magic, out-of-range field,
    /// truncated cell/varint, …).
    #[error("corrupt SQLite file: {0}")]
    Corrupt(String),
    /// A well-formed SQLite construct this crate deliberately does not support
    /// (WITHOUT ROWID, index b-trees, …).
    #[error("unsupported SQLite construct: {0}")]
    Unsupported(String),
    /// Underlying filesystem I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A lock-free WAL read was refused (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1).
    #[error("{0}")]
    Refused(#[from] SqliteReadRefusal),
}

/// Why a lock-free read of a live SQLite file was refused. The server maps each
/// refusal that has a wire token to its registered `ServerErrorCode` through [`SqliteReadRefusal::code`].
#[derive(Debug, Error)]
pub enum SqliteReadRefusal {
    /// A checkpoint changed the WAL salts during the read and the busy timeout elapsed
    /// before a stable snapshot was read.
    #[error("SQLITE_SNAPSHOT_UNSTABLE: the WAL changed during the read and did not settle")]
    SnapshotUnstable,
    /// The WAL or main file could not be read.
    #[error("io error reading the WAL snapshot: {0}")]
    Io(#[from] std::io::Error),
}

impl SqliteReadRefusal {
    /// The wire token of this refusal; the server's `ServerErrorCode` registers it.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::SnapshotUnstable => Some("SQLITE_SNAPSHOT_UNSTABLE"),
            Self::Io(_) => None,
        }
    }
}

impl Error {
    pub(crate) fn corrupt(msg: impl Into<String>) -> Self {
        Error::Corrupt(msg.into())
    }
    pub(crate) fn unsupported(msg: impl Into<String>) -> Self {
        Error::Unsupported(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
