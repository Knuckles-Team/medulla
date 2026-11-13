//! The pushdown seam: one remote source as the optimizer sees it, and the resolution of a
//! wire [`ForeignSourceSpec`] (or a registered name) to it.

use std::sync::Arc;

use medulla_query_types::wire::ForeignSourceSpec;

use super::capability::{KeyLookup, LimitPushdown, RemoteRequest, SourceCapabilities};
use super::snapshot::{SnapshotBound, SnapshotConn};
use super::stats::{fingerprint, short, Fingerprint};
use crate::federation::{ForeignSource, ForeignSourceRegistry, SharedForeignSource};
use medulla_catalog_types::metadata_acceleration::SnapshotSemantics;
use medulla_exec::budget::FederationBudget;
use medulla_exec::rowset::RowSet;
use medulla_catalog_types::consolidation_isolation::SourceQuota;

/// A foreign source that can answer a [`RemoteRequest`]. `fetch` must return a SUPERSET of
/// the rows the request selects: every row whose id is in `keys` (when keys are given), the
/// first `limit` rows of its own order (when a limit is given), or the given page.
pub trait RemoteFetch {
    fn capabilities(&self) -> SourceCapabilities;
    fn identity(&self) -> &Identity;
    /// `<kind>[:<name>]#<fingerprint>` — never a URL, DSN or credential.
    fn label(&self) -> String {
        self.identity().label.clone()
    }
    fn fingerprint(&self) -> Fingerprint {
        self.identity().fingerprint
    }
    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String>;
    /// The quota this source was registered under (MEDULLA-UNIFIED-DATA-PLANE-R031.2);
    /// `None` for an inline spec, which runs under the session's default quota.
    fn quota(&self) -> Option<SourceQuota> {
        None
    }
    /// A source safe to call on several threads. Opaque registered closures remain
    /// sequential because the registry's trait does not promise thread safety.
    fn parallel_safe(&self) -> Option<&(dyn RemoteFetch + Sync)> {
        None
    }
    /// How many of `keys` (a prefix) fit in one request; at least 1 when `keys` is non-empty.
    fn fit_keys(&self, keys: &[String]) -> usize {
        keys.len()
    }
    /// Whether `key` can be expressed in this source's key predicate at all.
    fn key_expressible(&self, _key: &str) -> bool {
        true
    }
    /// Open the query's one read-only snapshot of this source (MEDULLA-UNIFIED-DATA-PLANE-R038.3.2).
    /// `Ok(None)` for a source without snapshots. A declared replica whose replay lag is over
    /// `bound.replica_lag_bound_ms` refuses with `REPLICA_LAG_EXCEEDED` before any read.
    fn open_snapshot(&self, _bound: &SnapshotBound) -> Result<Option<SnapshotSemantics>, String> {
        Ok(None)
    }
    /// The connection [`Self::open_snapshot`] opened, for the session to hand to later fragments.
    fn held_snapshot(&self) -> Option<Arc<dyn SnapshotConn>> {
        None
    }
    /// Read through the connection an earlier fragment of this query opened.
    fn adopt_snapshot(&self, _conn: Arc<dyn SnapshotConn>) {}
}

/// The spec-derived identity of a source: its kind and fingerprint.
pub struct Identity {
    pub label: String,
    pub fingerprint: Fingerprint,
    pub cache_name: Option<String>,
}

impl Identity {
    /// Identity of a self-describing spec, optionally registered under `name`.
    pub fn of_spec(spec: &ForeignSourceSpec, name: Option<&str>) -> Self {
        let bytes = rmp_serde::to_vec_named(spec).unwrap_or_default();
        let fp = fingerprint(&bytes);
        let kind = match spec {
            ForeignSourceSpec::RemoteEngine { .. } => "remote-engine",
            ForeignSourceSpec::HttpJson { .. } => "http-json",
            ForeignSourceSpec::Sql { .. } => "sql",
            ForeignSourceSpec::SqliteFile { .. } => "sqlite-file",
            ForeignSourceSpec::Trino { .. }
            | ForeignSourceSpec::Cypher { .. }
            | ForeignSourceSpec::SparkBatch { .. }
            | ForeignSourceSpec::Metastore { .. }
            | ForeignSourceSpec::BrainGuarded { .. } => super::oq2::kind(spec).unwrap(),
            ForeignSourceSpec::Named { .. } => "named",
            ForeignSourceSpec::Api { .. } => "api",
            ForeignSourceSpec::Mcp { .. } => "mcp",
            ForeignSourceSpec::A2a { .. } => "a2a",
            ForeignSourceSpec::GraphQl { .. } => "graphql",
        };
        Self::labelled(kind, name, fp)
    }

    /// Identity of a registry-only source (a table/closure registered by name).
    fn of_registered(name: &str) -> Self {
        let fp = fingerprint(format!("named:{name}").as_bytes());
        Self::labelled("registered", Some(name), fp)
    }

    fn labelled(kind: &str, name: Option<&str>, fingerprint: Fingerprint) -> Self {
        let label = match name {
            Some(name) => format!("{kind}:{name}#{}", short(&fingerprint)),
            None => format!("{kind}#{}", short(&fingerprint)),
        };
        Self {
            label,
            fingerprint,
            cache_name: name.map(str::to_owned),
        }
    }
}

/// A source that can only be fetched whole: every request is answered by the full result
/// (its capabilities never let the optimizer ask for less).
pub(crate) struct Opaque<'a> {
    source: OpaqueSource<'a>,
    identity: Identity,
    /// The registration declared `key_filter_pushdown` for this source
    /// (MEDULLA-FEDERATED-QUERY-R722.8). Only a declared capability is ever pushed.
    key_pushdown: bool,
}

enum OpaqueSource<'a> {
    Spec(Box<dyn ForeignSource + 'a>),
    Registered(&'a SharedForeignSource),
}

impl RemoteFetch for Opaque<'_> {
    fn capabilities(&self) -> SourceCapabilities {
        if self.key_pushdown {
            return SourceCapabilities::single_full_fetch(
                KeyLookup::Batched {
                    max_keys: FederationBudget::default().max_bind_keys,
                },
                LimitPushdown::Unsupported,
            );
        }
        SourceCapabilities::fetch_only()
    }
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        let keys = (self.key_pushdown && !request.keys.is_empty()).then_some(&request.keys[..]);
        match (&self.source, keys) {
            (OpaqueSource::Spec(source), Some(keys)) => source.fetch_keys(keys),
            (OpaqueSource::Registered(source), Some(keys)) => source.fetch_keys(keys),
            (OpaqueSource::Spec(source), None) => source.fetch(),
            (OpaqueSource::Registered(source), None) => source.fetch(),
        }
    }
}

/// Resolve `spec` to the source the optimizer drives. `None` means the spec names a
/// source nothing can resolve — the caller reports it through the naive path, which owns
/// those error messages.
pub(crate) fn resolve<'a>(
    spec: &'a ForeignSourceSpec,
    registry: Option<&'a ForeignSourceRegistry>,
) -> Option<Box<dyn RemoteFetch + 'a>> {
    match spec {
        ForeignSourceSpec::Named { name } => resolve_named(name, registry?),
        other => Some(from_spec(other, None, None)),
    }
}

/// Resolve `spec` (or the spec a registered name holds) to a search-index source when its
/// capability declares `search` (MEDULLA-UNIFIED-DATA-PLANE-R038.5.3). A search source answers
/// a pushed search, not a row fetch, so it is not a [`RemoteFetch`]; `None` for every other spec.
pub(crate) fn resolve_search<'a>(
    spec: &'a ForeignSourceSpec,
    registry: Option<&'a ForeignSourceRegistry>,
) -> Option<Box<dyn super::search::SearchRemote + 'a>> {
    let (spec, name) = match spec {
        ForeignSourceSpec::Named { name } => (registry?.spec(name)?, Some(name.as_str())),
        other => (other, None),
    };
    match spec {
        ForeignSourceSpec::Mcp {
            server,
            tool,
            capabilities,
        } if capabilities.search.is_some() => Some(Box::new(
            super::mcp::McpRemote::new(
                (server, tool),
                (capabilities.key_filter_pushdown, capabilities.search),
                super::mcp::registered_port(),
                Identity::of_spec(spec, name),
            )
            .with_quota(name.and_then(|name| registry?.quota(name))),
        )),
        _ => None,
    }
}

/// A resolved source that carries the quota its registration named.
struct Quoted<'a> {
    inner: Box<dyn RemoteFetch + 'a>,
    quota: SourceQuota,
}

impl RemoteFetch for Quoted<'_> {
    fn capabilities(&self) -> SourceCapabilities {
        self.inner.capabilities()
    }
    fn identity(&self) -> &Identity {
        self.inner.identity()
    }
    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        self.inner.fetch(request)
    }
    fn quota(&self) -> Option<SourceQuota> {
        Some(self.quota)
    }
    fn parallel_safe(&self) -> Option<&(dyn RemoteFetch + Sync)> {
        self.inner.parallel_safe()
    }
    fn fit_keys(&self, keys: &[String]) -> usize {
        self.inner.fit_keys(keys)
    }
    fn key_expressible(&self, key: &str) -> bool {
        self.inner.key_expressible(key)
    }
    fn open_snapshot(&self, bound: &SnapshotBound) -> Result<Option<SnapshotSemantics>, String> {
        self.inner.open_snapshot(bound)
    }
    fn held_snapshot(&self) -> Option<Arc<dyn SnapshotConn>> {
        self.inner.held_snapshot()
    }
    fn adopt_snapshot(&self, conn: Arc<dyn SnapshotConn>) {
        self.inner.adopt_snapshot(conn);
    }
}

/// Resolve a registered name: its spec when it was registered as one (pushdown-capable),
/// else the registered source itself (fetch-only). A name registered with a quota carries it.
pub(crate) fn resolve_named<'a>(
    name: &'a str,
    registry: &'a ForeignSourceRegistry,
) -> Option<Box<dyn RemoteFetch + 'a>> {
    let quota = registry.quota(name);
    let remote = resolve_registered(name, registry, quota)?;
    Some(match quota {
        Some(quota) => Box::new(Quoted {
            inner: remote,
            quota,
        }),
        None => remote,
    })
}

fn resolve_registered<'a>(
    name: &'a str,
    registry: &'a ForeignSourceRegistry,
    quota: Option<SourceQuota>,
) -> Option<Box<dyn RemoteFetch + 'a>> {
    if let Some((spec, source)) = registry.bound_entry(name) {
        return Some(Box::new(Opaque {
            source: OpaqueSource::Registered(source),
            // The callback is the effective source, but the retained spec remains the
            // canonical identity/fingerprint used by traces and cache keys.
            identity: Identity::of_spec(spec, Some(name)),
            key_pushdown: declared_key_pushdown(spec),
        }));
    }
    if let Some(spec) = registry.spec(name) {
        return Some(from_spec(spec, Some(name), quota));
    }
    let source = registry.get(name)?;
    Some(Box::new(Opaque {
        source: OpaqueSource::Registered(source),
        identity: Identity::of_registered(name),
        key_pushdown: false,
    }))
}

/// Whether the registration of an operation-bound spec declared `key_filter_pushdown`
/// (MEDULLA-FEDERATED-QUERY-R722.8). Every other spec kind declares none here.
fn declared_key_pushdown(spec: &ForeignSourceSpec) -> bool {
    match spec {
        ForeignSourceSpec::Api { capabilities, .. }
        | ForeignSourceSpec::Mcp { capabilities, .. }
        | ForeignSourceSpec::A2a { capabilities, .. }
        | ForeignSourceSpec::GraphQl { capabilities, .. } => capabilities.key_filter_pushdown,
        _ => false,
    }
}

/// The pushdown-capable implementation for a self-describing spec.
fn from_spec<'a>(
    spec: &'a ForeignSourceSpec,
    name: Option<&'a str>,
    quota: Option<SourceQuota>,
) -> Box<dyn RemoteFetch + 'a> {
    let identity = Identity::of_spec(spec, name);
    match spec {
        ForeignSourceSpec::Sql {
            dsn,
            query,
            id_field,
            score_field,
            ..
        } => Box::new(super::sql::SqlRemote::new(
            super::sql::SqlSpec {
                dsn,
                query,
                id_field,
                score_field: score_field.as_deref(),
            },
            identity,
        )),
        ForeignSourceSpec::HttpJson {
            url,
            json_path,
            field_map,
        } => Box::new(super::http::HttpRemote::new(
            url, json_path, field_map, identity,
        )),
        ForeignSourceSpec::RemoteEngine { .. } => {
            Box::new(super::engine::EngineRemote::new(spec, identity))
        }
        ForeignSourceSpec::Mcp {
            server,
            tool,
            capabilities,
        } => Box::new(
            super::mcp::McpRemote::new(
                (server, tool),
                (capabilities.key_filter_pushdown, capabilities.search),
                super::mcp::registered_port(),
                identity,
            )
            .with_quota(quota),
        ),
        other => Box::new(Opaque {
            source: OpaqueSource::Spec(crate::federation::source_for(other)),
            identity,
            key_pushdown: declared_key_pushdown(other),
        }),
    }
}

#[cfg(test)]
mod bound_driver_tests {
    use super::{resolve_named, Identity, RemoteRequest};
    use crate::federation::{ForeignSource, ForeignSourceRegistry};
    use medulla_exec::rowset::RowSet;
    use medulla_query_types::wire::{ForeignOperationCapabilities, ForeignSourceSpec};
    use std::sync::Arc;

    struct BoundRows;

    impl ForeignSource for BoundRows {
        fn fetch(&self) -> Result<RowSet, String> {
            Ok(RowSet::from_ids(["bound-row".to_string()]))
        }
    }

    fn spec() -> ForeignSourceSpec {
        ForeignSourceSpec::Api {
            operation: "list_objects".into(),
            capabilities: ForeignOperationCapabilities::default(),
        }
    }

    /// MEDULLA-FEDERATED-QUERY-R722.3 — optimized resolution prefers the bound callback while
    /// retaining the spec-derived identity rather than the generic registered identity.
    // spec: MEDULLA-FEDERATED-QUERY-R722.9.1
    // spec: MEDULLA-FEDERATED-QUERY-R722.3
    #[test]
    fn bound_driver_precedes_spec_and_preserves_identity() {
        let spec = spec();
        let expected = Identity::of_spec(&spec, Some("objects"));
        let mut registry = ForeignSourceRegistry::new();
        registry.register_bound("objects", spec, Arc::new(BoundRows));

        let naive = registry
            .resolve("objects")
            .expect("bound source resolves naively");
        let remote = resolve_named("objects", &registry).expect("bound source resolves");
        assert_eq!(remote.identity().fingerprint, expected.fingerprint);
        assert_eq!(
            remote.capabilities(),
            super::super::capability::SourceCapabilities::fetch_only(),
            "a bound callback must not advertise pushdown before it has effective capabilities"
        );
        assert_eq!(
            remote.fetch(&RemoteRequest::full()).unwrap().ids(),
            naive.ids(),
            "naive and optimized resolution must drive the same bound source"
        );
    }

    /// MEDULLA-FEDERATED-QUERY-R722.4 — optimizer resolution keeps unbound operation specs on the
    /// typed refusing source instead of manufacturing a network callback.
    #[test]
    fn unbound_operation_spec_stays_fail_closed_in_optimized_path() {
        let mut registry = ForeignSourceRegistry::new();
        registry.register_spec("objects", spec());

        let remote = resolve_named("objects", &registry).expect("spec resolves to refusal source");
        let error = remote
            .fetch(&RemoteRequest::full())
            .expect_err("unbound operation must refuse");
        assert!(
            error.contains("requires a verified registration and bound driver"),
            "unexpected unbound refusal: {error}"
        );
    }
}
