//! Asset usage recorded from the server's own query dispatch (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1).
//!
//! After a SQL or unified query answers `Ok`, the datasets it referenced are counted once per
//! asset and verified principal in the `asset_usage.reads` and `asset_usage.queries` series of
//! `medulla-tsdb`. The principal and tenant come from the verified request carrier, never from a
//! field the client sends. A failed statement records nothing, and a recording failure never
//! fails the query: it counts `medulla_asset_usage_dropped_total`.
//!
//! The `asset_usage.queries` series also carries the question digest (MEDULLA-TYPED-PACKS-R108.8):
//! the SHA-256 of the statement text or plan bytes the caller sent, never a result row. The
//! `USAGE QUERIES` / `USAGE ASSETS` plan sources read these series back through
//! [`served_usage_leg`], over the trailing window `knob catalog.top_queries_window_days`.

use super::*;

/// The dataset names a query method referenced: SQL tables for `Method::Sql`, plan scan labels
/// for `Method::UnifiedQuery`. Any other method references none.
pub(crate) fn query_assets(method: &Method) -> Vec<String> {
    match method {
        #[cfg(feature = "federation-sql")]
        Method::Sql { query, .. } => medulla_federation::sql_analysis::referenced_tables(
            query,
            medulla_exec::sql_text::SqlDialect::Postgres,
        )
        .unwrap_or_default(),
        Method::UnifiedQuery { plan, .. } => plan_labels(plan),
        // A prepared UQL statement is planned, not run, so it reads nothing.
        Method::Uql { prepare: true, .. } => Vec::new(),
        // A UQL statement is read by the plan it parses to (MEDULLA-FEDERATED-QUERY-R722.5).
        Method::Uql { text, .. } => medulla_uql::parse(text)
            .map(|plan| plan_labels(&plan))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The distinct scan labels of a plan, sorted.
pub(super) fn plan_labels(plan: &medulla_query_types::wire::Plan) -> Vec<String> {
    let mut labels: Vec<String> = plan
        .ops
        .iter()
        .filter_map(|op| match op {
            medulla_query_types::wire::Op::Scan { label } => Some(label.clone()),
            // A pack source read counts as a read of its catalog asset
            // (MEDULLA-FEDERATED-QUERY-R722.5).
            #[cfg(feature = "federation")]
            medulla_query_types::wire::Op::ForeignScan { source, .. } => {
                foreign_source_name(source)
            }
            _ => None,
        })
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// The catalog asset a foreign scan reads: the registered source name, when the plan names one.
#[cfg(feature = "federation")]
fn foreign_source_name(source: &medulla_query_types::wire::ForeignSourceSpec) -> Option<String> {
    match source {
        medulla_query_types::wire::ForeignSourceSpec::Named { name } => Some(name.clone()),
        _ => None,
    }
}

/// The question digest of a query method (MEDULLA-TYPED-PACKS-R108.8): the SHA-256 of the SQL
/// or UQL text, or of the plan's MessagePack bytes, as the caller sent it. Never derived from
/// the answer. `None` for a method that asks no question.
pub(crate) fn question_digest(method: &Method) -> Option<String> {
    let bytes = match method {
        Method::Sql { query, .. } => query.as_bytes().to_vec(),
        Method::Uql { text, .. } => text.as_bytes().to_vec(),
        Method::UnifiedQuery { plan, .. } => rmp_serde::to_vec_named(plan).ok()?,
        _ => return None,
    };
    Some(medulla_types::contract::Digest256::sha256(&bytes).to_hex())
}

/// True when `response` is a successful answer.
pub(crate) fn answered(response: &Response) -> bool {
    response.error.is_none()
}

/// Record one read and one query for each of `assets` by the verified caller; the query point
/// carries the question `digest` when the method has one. Never fails the query: a dropped
/// point is counted instead.
#[cfg(feature = "tsdb")]
pub(crate) async fn record_query_usage(
    ctx: &QueryHandlerCtx<'_>,
    assets: Vec<String>,
    digest: Option<String>,
) {
    use medulla_catalog_types::asset_usage::{
        catalog_usage_period_s, usage_query_series_name, usage_series_name,
        ASSET_USAGE_QUERIES_SERIES, ASSET_USAGE_READS_SERIES,
    };
    use medulla_tsdb::point::Point;
    use medulla_tsdb::store::SeriesKey;

    if assets.is_empty() {
        return;
    }
    let Some(carrier) = ctx.read_authority.and_then(|authority| authority.carrier()) else {
        return;
    };
    let store = ctx.state.read().await.tsdb_store.clone();
    let Some(store) = store else {
        medulla_kernel::metrics::process().asset_usage_dropped();
        return;
    };
    let bucket_ns = catalog_usage_period_s().saturating_mul(1_000_000_000);
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    let tenant = carrier.tenant_scope();
    let principal = carrier.effective_actor_scope();
    let fields = vec!["count".to_string()];
    for asset in &assets {
        let query_series = match digest.as_deref() {
            Some(digest) => usage_query_series_name(principal, asset, digest),
            None => usage_series_name(ASSET_USAGE_QUERIES_SERIES, principal, asset),
        };
        for name in [
            usage_series_name(ASSET_USAGE_READS_SERIES, principal, asset),
            query_series,
        ] {
            let key = SeriesKey::new(tenant, ctx.graph_name, name);
            let point = [Point::single(now_ns, 1.0)];
            if store
                .append_scoped(&key, 1, bucket_ns, &fields, &point)
                .is_err()
            {
                medulla_kernel::metrics::process().asset_usage_dropped();
            }
        }
    }
}

/// Without the time-series store nothing can be recorded; the points count as dropped.
#[cfg(not(feature = "tsdb"))]
pub(crate) async fn record_query_usage(
    _ctx: &QueryHandlerCtx<'_>,
    assets: Vec<String>,
    _digest: Option<String>,
) {
    for _ in assets {
        medulla_kernel::metrics::process().asset_usage_dropped();
    }
}

/// The caller's usage log for a plan with a `USAGE` source (MEDULLA-TYPED-PACKS-R108.8): the
/// usage series of the verified carrier's tenant and the queried graph, over the trailing
/// window `knob catalog.top_queries_window_days`. `None` when the plan reads no usage.
#[cfg(feature = "tsdb")]
pub(crate) async fn served_usage_leg(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    read_authority: Option<&GraphReadAuthority>,
    plan: &medulla_query_types::wire::Plan,
) -> Result<Option<Arc<dyn medulla_exec::exec::UsageSource>>, String> {
    if !super::planning::plan_needs_usage(&plan.ops) {
        return Ok(None);
    }
    let Some(carrier) = read_authority.and_then(GraphReadAuthority::carrier) else {
        medulla_kernel::metrics::process().access_denied();
        return Err("ACCESS_DENIED: USAGE requires a verified tenant carrier".to_string());
    };
    let Some(store) = state.read().await.tsdb_store.clone() else {
        return Err("USAGE: the usage log is unavailable on this server".to_string());
    };
    let days = medulla_server_api::knob_config::knobs().catalog_top_queries_window_days;
    let to_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let window = medulla_exec::exec::UsageWindow {
        from_s: to_s.saturating_sub(days.saturating_mul(SECONDS_PER_DAY)),
        to_s,
    };
    Ok(Some(Arc::new(ServedUsage {
        store,
        tenant: carrier.tenant_scope().to_string(),
        graph: graph_name.to_string(),
        window,
    })))
}

/// Without the time-series store no usage is recorded, so a `USAGE` source cannot be answered.
#[cfg(not(feature = "tsdb"))]
pub(crate) async fn served_usage_leg(
    _state: &Arc<RwLock<ServerState>>,
    _graph_name: &str,
    _read_authority: Option<&GraphReadAuthority>,
    plan: &medulla_query_types::wire::Plan,
) -> Result<Option<Arc<dyn medulla_exec::exec::UsageSource>>, String> {
    if super::planning::plan_needs_usage(&plan.ops) {
        return Err("USAGE: the usage log is unavailable in this build".to_string());
    }
    Ok(None)
}

/// Seconds in one day, the unit of `knob catalog.top_queries_window_days`.
#[cfg(feature = "tsdb")]
const SECONDS_PER_DAY: u64 = 86_400;
/// Nanoseconds in one second, the unit the usage points are stamped in.
#[cfg(feature = "tsdb")]
const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// The usage series of one tenant and graph, read over one window.
#[cfg(feature = "tsdb")]
struct ServedUsage {
    store: Arc<medulla_tsdb::store::SeriesStore>,
    tenant: String,
    graph: String,
    window: medulla_exec::exec::UsageWindow,
}

#[cfg(feature = "tsdb")]
impl medulla_exec::exec::UsageSource for ServedUsage {
    fn window(&self) -> medulla_exec::exec::UsageWindow {
        self.window
    }

    fn usage_series(&self, base: &str) -> Result<Vec<medulla_exec::exec::UsageSeries>, String> {
        use medulla_tsdb::store::SeriesKey;
        let unavailable = |_| "ENGINE_UNAVAILABLE: the usage store could not be read".to_string();
        let catalog = self
            .store
            .scoped_catalog(&self.tenant, &self.graph)
            .map_err(unavailable)?;
        let to_s = |s: u64| i64::try_from(s).unwrap_or(i64::MAX);
        let from_ns = to_s(self.window.from_s).saturating_mul(NANOS_PER_SECOND);
        // The window's last second is inclusive, so the scan ends one second after it.
        let to_ns = to_s(self.window.to_s)
            .saturating_add(1)
            .saturating_mul(NANOS_PER_SECOND);
        let mut out = Vec::new();
        for name in catalog.into_keys() {
            if medulla_catalog_types::asset_usage::usage_series_tags(&name, base).is_none() {
                continue;
            }
            let key = SeriesKey::new(&self.tenant, &self.graph, name.clone());
            let points = self
                .store
                .range_scoped(&key, from_ns, to_ns)
                .map_err(unavailable)?;
            let uses: f64 = points
                .iter()
                .map(|p| p.values.first().copied().unwrap_or(0.0))
                .sum();
            let last_ns = points.iter().map(|p| p.ts).max().unwrap_or(0);
            if uses < 1.0 {
                continue;
            }
            out.push(medulla_exec::exec::UsageSeries {
                name,
                uses: uses as u64,
                last_used_s: u64::try_from(last_ns / NANOS_PER_SECOND).unwrap_or(0),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.3.1
    #[test]
    fn unified_plan_scan_labels_are_the_assets() {
        use medulla_query_types::wire::{Op, Plan};
        let plan = Plan::new(vec![
            Op::Scan {
                label: "Doc".into(),
            },
            Op::Limit { k: 3 },
            Op::Scan {
                label: "Doc".into(),
            },
            Op::Scan {
                label: "Person".into(),
            },
        ]);
        assert_eq!(plan_labels(&plan), vec!["Doc", "Person"]);
        assert!(query_assets(&Method::Ping).is_empty());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.3.1
    #[test]
    fn a_failed_answer_records_nothing() {
        assert!(!answered(&Response::err(1, "SQL_ERROR: nope")));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R722.5
    #[test]
    fn a_prepared_uql_statement_reads_no_asset() {
        let uql = |prepare| {
            serde_json::from_value::<Method>(serde_json::json!({
                "method": "Uql",
                "params": {"text": "MATCH (:Doc)", "prepare": prepare},
            }))
            .expect("a Uql method")
        };
        assert!(query_assets(&uql(true)).is_empty());
        assert_eq!(query_assets(&uql(false)), vec!["Doc"]);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.8
    #[test]
    fn the_question_digest_is_the_sha256_of_what_the_caller_sent() {
        let uql = serde_json::from_value::<Method>(serde_json::json!({
            "method": "Uql",
            "params": {"text": "MATCH (:Doc)", "prepare": false},
        }))
        .expect("a Uql method");
        assert_eq!(
            question_digest(&uql).as_deref(),
            Some(
                medulla_types::contract::Digest256::sha256(b"MATCH (:Doc)")
                    .to_hex()
                    .as_str()
            )
        );
        assert!(question_digest(&Method::Ping).is_none());
    }
}
