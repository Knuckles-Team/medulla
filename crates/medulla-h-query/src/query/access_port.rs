//! The read-time access check port over a connector's own permission check
//! (MEDULLA-FEDERATED-QUERY-R703.3).
//!
//! [`ForeignAccessCheckPort`] implements the engine's [`AccessCheckPort`]. A hit's
//! `source` names one of the caller's registered operation-bound sources; the check
//! goes to that source's connector through the host-registered connector port (the
//! same request-scoped driver path `ForeignScan` rows take), calling the connector
//! tool [`ACCESS_CHECK_TOOL`]. A source with no registered operation-bound
//! registration, a build with no connector port, a failed call, or an answer that
//! names a record twice all report "cannot answer", and the caller drops the hits.

use std::collections::BTreeMap;
use std::time::Instant;

use medulla_exec::access_check::{
    AccessCheckAnswer, AccessCheckPort, AccessCheckRequest, CHECK_TIMEOUT_MS,
};

/// The connector tool that answers `access_check`.
pub(crate) const ACCESS_CHECK_TOOL: &str = "source_access_check";

/// Checks sent to connectors, keyed by hit source name to the connector server.
pub(crate) struct ForeignAccessCheckPort {
    servers: BTreeMap<String, String>,
}

impl ForeignAccessCheckPort {
    pub(crate) fn new(servers: BTreeMap<String, String>) -> Self {
        Self { servers }
    }
}

impl AccessCheckPort for ForeignAccessCheckPort {
    #[cfg(feature = "federation")]
    fn check(&mut self, request: &AccessCheckRequest) -> Option<(AccessCheckAnswer, u64)> {
        use medulla_federation::federation_opt::mcp::{registered_port, ConnectorAccessCheck};
        let server = self.servers.get(&request.source)?;
        let port = registered_port()?;
        let started = Instant::now();
        let answer = port
            .access_check(&ConnectorAccessCheck {
                server: server.clone(),
                tool: ACCESS_CHECK_TOOL.to_string(),
                principal: request.principal.clone(),
                record_refs: request.record_refs.clone(),
                timeout_ms: CHECK_TIMEOUT_MS,
            })
            .ok()?;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Some((
            AccessCheckAnswer {
                allowed: answer.allowed,
                denied: answer.denied,
                checked_at: answer.checked_at,
            },
            elapsed_ms,
        ))
    }

    #[cfg(not(feature = "federation"))]
    fn check(&mut self, request: &AccessCheckRequest) -> Option<(AccessCheckAnswer, u64)> {
        let _ = (&self.servers, request, Instant::now, CHECK_TIMEOUT_MS);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R703.3
    #[test]
    fn unregistered_source_cannot_answer() {
        let mut port = ForeignAccessCheckPort::new(BTreeMap::new());
        let request = AccessCheckRequest {
            source: "unregistered".into(),
            principal: "alice".into(),
            record_refs: vec!["doc-1".into()],
        };
        assert!(port.check(&request).is_none());
    }
}
