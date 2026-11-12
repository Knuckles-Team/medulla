//! Asset usage recorded from the server's own query dispatch (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1).
//!
//! After a SQL or unified query answers `Ok`, the datasets it referenced are counted once per
//! asset and verified principal in the `asset_usage.reads` and `asset_usage.queries` series of
//! `medulla-tsdb`. The principal and tenant come from the verified request carrier, never from a
//! field the client sends. A failed statement records nothing, and a recording failure never
//! fails the query: it counts `medulla_asset_usage_dropped_total`.

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
        _ => Vec::new(),
    }
}

/// The distinct scan labels of a plan, sorted.
fn plan_labels(plan: &medulla_exec::algebra::Plan) -> Vec<String> {
    let mut labels: Vec<String> = plan
        .ops
        .iter()
        .filter_map(|op| match op {
            medulla_exec::algebra::Op::Scan { label } => Some(label.clone()),
            _ => None,
        })
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// True when `response` is a successful answer.
pub(crate) fn answered(response: &Response) -> bool {
    response.error.is_none()
}

/// Record one read and one query for each of `assets` by the verified caller. Never fails the
/// query: a dropped point is counted instead.
#[cfg(feature = "tsdb")]
pub(crate) async fn record_query_usage(ctx: &QueryHandlerCtx<'_>, assets: Vec<String>) {
    use medulla_tsdb::point::Point;
    use medulla_tsdb::store::SeriesKey;
    use medulla_types::asset_usage::{
        catalog_usage_period_s, usage_series_name, ASSET_USAGE_QUERIES_SERIES,
        ASSET_USAGE_READS_SERIES,
    };

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
        for base in [ASSET_USAGE_READS_SERIES, ASSET_USAGE_QUERIES_SERIES] {
            let key = SeriesKey::new(
                tenant,
                ctx.graph_name,
                usage_series_name(base, principal, asset),
            );
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
pub(crate) async fn record_query_usage(_ctx: &QueryHandlerCtx<'_>, assets: Vec<String>) {
    for _ in assets {
        medulla_kernel::metrics::process().asset_usage_dropped();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.3.1
    #[test]
    fn unified_plan_scan_labels_are_the_assets() {
        use medulla_exec::algebra::{Op, Plan};
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
}
