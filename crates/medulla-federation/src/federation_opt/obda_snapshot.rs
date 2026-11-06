//! One snapshot per external SQL source for a whole OBDA request (MEDULLA-UNIFIED-DATA-PLANE-R038.3.6).
//!
//! `sparql_virtual` opens one [`FederationSession`] for the request. Every scan of one source
//! reads through the snapshot the first scan opened (the same bookkeeping a `UnifiedQuery`
//! uses in `run.rs`), so a writer that commits between two scans cannot split the answer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::session::FederationSession;
use super::stats::fingerprint;

/// The reader of one OBDA request: its session and the lock that serializes snapshot opening.
pub struct ObdaSnapshotReader {
    session: Arc<FederationSession>,
    open: Mutex<()>,
}

impl ObdaSnapshotReader {
    /// A reader over the request's `session`.
    pub fn new(session: Arc<FederationSession>) -> Self {
        Self {
            session,
            open: Mutex::new(()),
        }
    }

    /// The request's session, for the trace.
    pub fn session(&self) -> &Arc<FederationSession> {
        &self.session
    }

    /// Run one read statement against `dsn` and return each row as column to text value.
    /// A PostgreSQL source reads through the request's one snapshot; any other source reads
    /// as before.
    pub fn fetch_columns(
        &self,
        dsn: &str,
        sql: &str,
    ) -> Result<Vec<HashMap<String, String>>, String> {
        let key = fingerprint(format!("obda-sql:{dsn}").as_bytes());
        let _serial = self
            .open
            .lock()
            .map_err(|_| "federation: obda snapshot lock poisoned".to_string())?;
        if let Some(handle) = self.session.snapshot_of(&key) {
            if let Some(conn) = handle.conn {
                return conn.run_columns(sql);
            }
            return crate::federation::fetch_sql_columns(dsn, sql);
        }
        if medulla_exec::sql_text::SqlDialect::from_dsn(dsn)
            == Some(medulla_exec::sql_text::SqlDialect::Postgres)
        {
            use super::snapshot::{SnapshotConn, SnapshotHandle};
            use medulla_types::metadata_acceleration::{SnapshotIsolation, SnapshotSemantics};
            let bound = self.session.snapshot_bound();
            let (conn, token) = crate::federation::PgSnapshot::open(dsn, "id", None, &bound)?;
            let conn: Arc<dyn SnapshotConn> = Arc::new(conn);
            self.session.hold_snapshot(
                key,
                SnapshotHandle {
                    semantics: SnapshotSemantics {
                        isolation: SnapshotIsolation::RepeatableRead,
                        snapshot_token: token,
                        replica_lag_bound_ms: bound.replica_lag_bound_ms,
                    },
                    conn: Some(Arc::clone(&conn)),
                },
            );
            return conn.run_columns(sql);
        }
        crate::federation::fetch_sql_columns(dsn, sql)
    }
}
