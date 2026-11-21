//! Query cancellation over the PostgreSQL wire (MEDULLA-DURABLE-KERNEL-R035.5).
//!
//! The wire layer's [`ConnectionManager`] is the one registry of live connections,
//! keyed by `(pid, secret_key)`. Login registers each connection and keeps the
//! returned handle and guard in the client's session extensions; the guard
//! unregisters on disconnect. A cancel request arrives on its own connection and
//! reaches [`EngineCancelHandler`], which holds no state: it delegates to the
//! manager and logs a miss, so a wrong secret changes nothing.
//!
//! pgwire's own `on_query` and `on_execute` take `ConnectionHandle::start_query()`
//! before they call `do_query` and race `do_query` against the receiver; when the
//! receiver fires they drop the `do_query` future and answer SQLSTATE `57014`. A
//! second `start_query()` inside `do_query` would replace that sender and end every
//! statement as cancelled, so `do_query` instead holds a [`StatementCancel`]: it
//! installs the statement's [`medulla_sql::sql::CancellationToken`] on the session,
//! and when the future is dropped mid-statement it fires the token, which stops the
//! running read at its next batch boundary.

use std::sync::Arc;

use async_trait::async_trait;
use pgwire::api::cancel::CancelHandler;
use pgwire::api::ConnectionManager;
use pgwire::messages::cancel::CancelRequest;

use medulla_wire_core::wire::WireSession;

/// The engine's cancel handler: it receives a cancel request and delegates it to the
/// listener's [`ConnectionManager`].
pub(super) struct EngineCancelHandler {
    connections: Arc<ConnectionManager>,
}

impl EngineCancelHandler {
    pub(super) fn new(connections: Arc<ConnectionManager>) -> Self {
        Self { connections }
    }
}

#[async_trait]
impl CancelHandler for EngineCancelHandler {
    async fn on_cancel_request(&self, cancel_request: CancelRequest) {
        if !self
            .connections
            .cancel(cancel_request.pid, &cancel_request.secret_key)
            .await
        {
            tracing::warn!(
                pid = cancel_request.pid,
                "pgwire cancel request matched no live connection; ignored"
            );
        }
    }
}

/// One running statement's cancellation scope. Built before the statement runs and
/// finished after it returns. Dropped unfinished (pgwire dropped the `do_query`
/// future because a cancel request fired), it cancels the statement's token.
pub(super) struct StatementCancel {
    session: Arc<WireSession>,
    finished: bool,
}

impl StatementCancel {
    /// Install a fresh token for the statement about to run on `session`.
    pub(super) fn begin(session: &Arc<WireSession>) -> Self {
        session.begin_statement();
        Self {
            session: Arc::clone(session),
            finished: false,
        }
    }

    /// The statement returned: end it without cancelling.
    pub(super) fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for StatementCancel {
    fn drop(&mut self) {
        if let Some(token) = self.session.end_statement() {
            if !self.finished {
                token.cancel();
            }
        }
    }
}
