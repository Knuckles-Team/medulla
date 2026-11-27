//! Event-stream sources (MEDULLA-FEDERATED-QUERY-R073.2).
//!
//! The matrix row of an event stream permits a time range and nothing else
//! ([`PushdownKind::TimeRange`](super::capability::PushdownKind)). [`StreamRemote`] therefore
//! sends the source's window `[from_ms, to_ms)` as the only arguments of its tool, whatever the
//! request carries: a key lookup, a limit or a filter is never pushed, and its capabilities say
//! so, so the planner keeps all of those as the local residual. The call itself goes through
//! the connector port exactly as an MCP source's does.

use std::collections::BTreeMap;

use medulla_catalog_source_types::consolidation_isolation::SourceQuota;
use medulla_exec::rowset::RowSet;
use serde_json::{json, Value};

use super::capability::{
    ConnectionProtocol, KeyLookup, LimitPushdown, RemoteRequest, SourceCapabilities,
};
use super::mcp::{McpRemote, SharedPort};
use super::remote::{Identity, RemoteFetch};

/// An event-stream source resolved to the connector port.
pub(crate) struct StreamRemote {
    inner: McpRemote,
    from_ms: Option<u64>,
    to_ms: Option<u64>,
}

impl StreamRemote {
    pub(crate) fn new(
        (server, tool): (&str, &str),
        (from_ms, to_ms): (Option<u64>, Option<u64>),
        port: Option<SharedPort>,
        identity: Identity,
    ) -> Self {
        Self {
            inner: McpRemote::new((server, tool), (false, None), port, identity),
            from_ms,
            to_ms,
        }
    }

    /// Bound each call by the registration's quota.
    pub(crate) fn with_quota(mut self, quota: Option<SourceQuota>) -> Self {
        self.inner = self.inner.with_quota(quota);
        self
    }

    /// The tool arguments: the time range, each end only when the window has it.
    pub(crate) fn arguments(&self) -> BTreeMap<String, Value> {
        let mut arguments = BTreeMap::new();
        if let Some(from) = self.from_ms {
            arguments.insert("from_ms".to_string(), json!(from));
        }
        if let Some(to) = self.to_ms {
            arguments.insert("to_ms".to_string(), json!(to));
        }
        arguments
    }
}

impl RemoteFetch for StreamRemote {
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::single_full_fetch(KeyLookup::Unsupported, LimitPushdown::Unsupported)
    }

    fn protocol(&self) -> Option<ConnectionProtocol> {
        Some(ConnectionProtocol::EventStream)
    }

    fn identity(&self) -> &Identity {
        <McpRemote as RemoteFetch>::identity(&self.inner)
    }

    fn fetch(&self, _request: &RemoteRequest) -> Result<RowSet, String> {
        // The request's keys and limit are dropped on purpose: only the time range is pushed.
        self.inner.call(self.arguments(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation_opt::capability::{in_matrix, PushdownKind};

    fn remote(from_ms: Option<u64>, to_ms: Option<u64>) -> StreamRemote {
        let spec = medulla_query_types::wire::ForeignSourceSpec::EventStream {
            server: "events-mcp".into(),
            tool: "read_events".into(),
            from_ms,
            to_ms,
            capabilities: Default::default(),
        };
        StreamRemote::new(
            ("events-mcp", "read_events"),
            (from_ms, to_ms),
            None,
            Identity::of_spec(&spec, None),
        )
    }

    // spec: MEDULLA-FEDERATED-QUERY-R073.2
    #[test]
    fn a_stream_pushes_its_time_range_and_nothing_else() {
        let source = remote(Some(1_000), Some(2_000));
        assert_eq!(source.protocol(), Some(ConnectionProtocol::EventStream));
        assert_eq!(
            source.arguments(),
            BTreeMap::from([
                ("from_ms".to_string(), json!(1_000)),
                ("to_ms".to_string(), json!(2_000))
            ])
        );
        assert_eq!(
            remote(Some(5), None).arguments(),
            BTreeMap::from([("from_ms".to_string(), json!(5))])
        );
        assert!(remote(None, None).arguments().is_empty());
        // The row permits a time range and no key lookup, limit or filter.
        let caps = source.capabilities();
        assert_eq!(caps.key_lookup, KeyLookup::Unsupported);
        assert_eq!(caps.limit, LimitPushdown::Unsupported);
        let protocol = source.protocol();
        assert!(in_matrix(protocol, PushdownKind::TimeRange));
        for kind in [
            PushdownKind::BatchedLookup,
            PushdownKind::LimitOrder,
            PushdownKind::Filter,
            PushdownKind::Projection,
        ] {
            assert!(!in_matrix(protocol, kind), "{kind:?} is outside the row");
        }
        assert_eq!(
            source.identity().label.split('#').next(),
            Some("event-stream")
        );
    }
}
