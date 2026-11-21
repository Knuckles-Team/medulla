//! The live-session registry behind `pg_stat_activity` (MEDULLA-DURABLE-KERNEL-R035.3.2).
//!
//! One [`WireActivity`] per listener holds one [`ActivityRow`] per connected session, keyed by the
//! backend-key pid the login handed the client. A session registers its row at startup and holds
//! the returned [`ActivityGuard`]; each statement sets the row `Active` with the first
//! [`MAX_ACTIVITY_QUERY_BYTES`] bytes of its text and returns it to `Idle` or `IdleInTransaction`
//! when it finishes; dropping the guard removes the row. A read maps the rows its caller may see
//! with [`WireActivity::visible_to`]: the caller's tenant, and without `kg:admin` only the
//! caller's own principal.

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// The longest statement text a row carries, in bytes.
pub const MAX_ACTIVITY_QUERY_BYTES: usize = 1024;

/// What a session is doing, rendered as `pg_stat_activity.state` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Idle,
    Active,
    IdleInTransaction,
}

impl ActivityState {
    /// The `pg_stat_activity.state` text of this state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
            Self::IdleInTransaction => "idle in transaction",
        }
    }
}

/// One registered session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRow {
    pub pid: i32,
    pub tenant: String,
    pub principal: String,
    pub graph: String,
    pub state: ActivityState,
    pub query_start_ms: i64,
    pub query: String,
}

/// The listener's registry of live sessions.
#[derive(Debug, Default)]
pub struct WireActivity {
    rows: Mutex<BTreeMap<i32, ActivityRow>>,
}

/// Removes its session's row when dropped.
#[derive(Debug)]
pub struct ActivityGuard {
    activity: Arc<WireActivity>,
    pid: i32,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// `query` cut to [`MAX_ACTIVITY_QUERY_BYTES`] at a character boundary.
fn bounded_query(query: &str) -> String {
    let mut end = query.len().min(MAX_ACTIVITY_QUERY_BYTES);
    while !query.is_char_boundary(end) {
        end -= 1;
    }
    query[..end].to_string()
}

impl WireActivity {
    /// Register the session `pid` as `Idle` and return the guard that removes it.
    pub fn register(
        self: &Arc<Self>,
        pid: i32,
        tenant: String,
        principal: String,
        graph: String,
    ) -> ActivityGuard {
        self.rows.lock().insert(
            pid,
            ActivityRow {
                pid,
                tenant,
                principal,
                graph,
                state: ActivityState::Idle,
                query_start_ms: now_ms(),
                query: String::new(),
            },
        );
        ActivityGuard {
            activity: Arc::clone(self),
            pid,
        }
    }

    /// The rows `tenant`'s caller `principal` may read: every row of the tenant when
    /// `admin`, otherwise only the principal's own.
    pub fn visible_to(&self, tenant: &str, principal: &str, admin: bool) -> Vec<ActivityRow> {
        self.rows
            .lock()
            .values()
            .filter(|row| row.tenant == tenant && (admin || row.principal == principal))
            .cloned()
            .collect()
    }
}

impl ActivityGuard {
    /// The session starts a statement: `Active`, with the bounded statement text.
    pub fn begin_statement(&self, graph: &str, query: &str) {
        if let Some(row) = self.activity.rows.lock().get_mut(&self.pid) {
            row.state = ActivityState::Active;
            row.graph = graph.to_string();
            row.query_start_ms = now_ms();
            row.query = bounded_query(query);
        }
    }

    /// The statement finished: back to `Idle`, or `IdleInTransaction` inside a transaction.
    pub fn end_statement(&self, in_transaction: bool) {
        if let Some(row) = self.activity.rows.lock().get_mut(&self.pid) {
            row.state = if in_transaction {
                ActivityState::IdleInTransaction
            } else {
                ActivityState::Idle
            };
        }
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.activity.rows.lock().remove(&self.pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_pids(rows: &[ActivityRow]) -> Vec<i32> {
        rows.iter().map(|row| row.pid).collect()
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.3.2
    #[test]
    fn guard_drop_removes_the_row_and_filter_scopes_by_principal() {
        let activity = Arc::new(WireActivity::default());
        let alice = activity.register(1, "t1".into(), "alice".into(), "g".into());
        let bob = activity.register(2, "t1".into(), "bob".into(), "g".into());
        let _other = activity.register(3, "t2".into(), "alice".into(), "g".into());

        // A plain caller sees only its own principal's rows inside its tenant.
        assert_eq!(row_pids(&activity.visible_to("t1", "alice", false)), vec![1]);
        // An admin sees every row of its own tenant and no other tenant's.
        assert_eq!(row_pids(&activity.visible_to("t1", "alice", true)), vec![1, 2]);

        // A statement is active with its bounded text, then idle, or idle in a transaction.
        let long = "é".repeat(MAX_ACTIVITY_QUERY_BYTES);
        alice.begin_statement("g2", &long);
        let row = activity.visible_to("t1", "alice", false).remove(0);
        assert_eq!(row.state, ActivityState::Active);
        assert_eq!(row.state.as_str(), "active");
        assert_eq!(row.graph, "g2");
        assert!(row.query.len() <= MAX_ACTIVITY_QUERY_BYTES && row.query.starts_with('é'));
        alice.end_statement(true);
        let row = activity.visible_to("t1", "alice", false).remove(0);
        assert_eq!(row.state.as_str(), "idle in transaction");
        alice.end_statement(false);
        assert_eq!(
            activity.visible_to("t1", "alice", false)[0].state.as_str(),
            "idle"
        );

        // Dropping a guard removes exactly its row.
        drop(alice);
        assert!(activity.visible_to("t1", "alice", false).is_empty());
        assert_eq!(row_pids(&activity.visible_to("t1", "bob", false)), vec![2]);
        drop(bob);
        assert!(activity.visible_to("t1", "bob", true).is_empty());
    }
}
