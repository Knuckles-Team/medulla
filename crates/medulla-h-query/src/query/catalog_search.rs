//! `Method::CatalogUsage` (MEDULLA-UNIFIED-DATA-PLANE-R764.8): who reads an asset and which
//! joins and filters are common on it.
//!
//! The answer is built from the usage series of the caller's own tenant and graph
//! (`asset_usage.reads`, `.join` and `.filter`), which the server's own query dispatch
//! (R764.3.1) and the source query log runs (R764.7) write. An asset with no usage series in
//! that scope is `CATALOG_ASSET_NOT_FOUND`, which is also what an asset of another tenant
//! looks like. Principal identities appear only for a caller holding scope
//! `catalog:usage:principals`; other callers get the counts with `principal` null.

use super::*;
use medulla_protocol_types::result_contract::query::CatalogUsage;
use medulla_types::asset_usage::{
    CatalogUsageAnswer, FrequentUser, UsageSuggestion, ASSET_USAGE_FILTER_SERIES,
    ASSET_USAGE_JOIN_SERIES, ASSET_USAGE_READS_SERIES, CATALOG_USAGE_PRINCIPALS_SCOPE,
    CATALOG_USAGE_SUGGESTION_MIN_COUNT, CATALOG_USAGE_TOP_N,
};

/// Refusal code: the asset has no usage in the caller's scope.
pub(crate) const CATALOG_ASSET_NOT_FOUND: &str = "CATALOG_ASSET_NOT_FOUND";
/// Refusal code: `top_n` lies outside the knob's bounds.
pub(crate) const CATALOG_LIMIT_INVALID: &str = "CATALOG_LIMIT_INVALID";

/// The tags of one usage series name, when it is a series of `base` about `asset`: the
/// principal and the rest of the tags after the asset.
fn series_tags<'a>(series: &'a str, base: &str, asset: &str) -> Option<(&'a str, &'a str)> {
    let rest = series.strip_prefix(base)?.strip_prefix("|principal=")?;
    let (principal, after) = rest.split_once("|asset=")?;
    let tail = after.strip_prefix(asset)?;
    if tail.is_empty() {
        Some((principal, ""))
    } else {
        tail.strip_prefix('|').map(|tags| (principal, tags))
    }
}

/// Rank `counts` (count, then name) and keep `top_n`.
fn ranked(counts: std::collections::BTreeMap<String, u64>, top_n: usize) -> Vec<(String, u64)> {
    let mut all: Vec<(String, u64)> = counts.into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    all.truncate(top_n);
    all
}

/// Build the answer from `(series name, summed count)` pairs of one tenant and graph. Pure.
pub(crate) fn usage_answer(
    asset: &str,
    series: &[(String, u64)],
    top_n: usize,
    min_count: u64,
    show_principals: bool,
) -> Result<CatalogUsageAnswer, String> {
    let mut readers = std::collections::BTreeMap::new();
    let mut joins = std::collections::BTreeMap::new();
    let mut filters = std::collections::BTreeMap::new();
    let mut known = false;
    for (name, count) in series {
        if let Some((principal, _)) = series_tags(name, ASSET_USAGE_READS_SERIES, asset) {
            known = true;
            *readers.entry(principal.to_string()).or_insert(0u64) += count;
        } else if let Some((_, tags)) = series_tags(name, ASSET_USAGE_JOIN_SERIES, asset) {
            known = true;
            if let Some(with) = tags.strip_prefix("with=") {
                *joins.entry(with.to_string()).or_insert(0u64) += count;
            }
        } else if let Some((_, tags)) = series_tags(name, ASSET_USAGE_FILTER_SERIES, asset) {
            known = true;
            if let Some(column) = tags.strip_prefix("column=") {
                *filters.entry(column.to_string()).or_insert(0u64) += count;
            }
        }
    }
    if !known {
        return Err(format!(
            "{CATALOG_ASSET_NOT_FOUND}: no usage is recorded for this asset"
        ));
    }
    let frequent_users = ranked(readers, top_n)
        .into_iter()
        .map(|(principal, reads)| FrequentUser {
            principal: show_principals.then_some(principal),
            reads,
        })
        .collect();
    let mut suggestions = Vec::new();
    for (kind, mut counts) in [("join", joins), ("filter", filters)] {
        counts.retain(|_, count| *count >= min_count);
        for (target, count) in ranked(counts, top_n) {
            suggestions.push(UsageSuggestion {
                kind: kind.to_string(),
                target,
                count,
            });
        }
    }
    Ok(CatalogUsageAnswer {
        asset: asset.to_string(),
        frequent_users,
        suggestions,
    })
}

/// Serve one `CatalogUsage` request.
pub(crate) async fn handle_usage(
    ctx: &QueryHandlerCtx<'_>,
    asset: String,
    top_n: Option<u32>,
) -> Response {
    let (min, max) = CATALOG_USAGE_TOP_N.range;
    let top_n = match top_n {
        None => CATALOG_USAGE_TOP_N.value(),
        Some(n) if (min..=max).contains(&(n as u64)) => n as u64,
        Some(_) => {
            return Response::err(
                ctx.req_id,
                format!("{CATALOG_LIMIT_INVALID}: top_n must be between {min} and {max}"),
            )
        }
    };
    let Some(carrier) = ctx.read_authority.and_then(|authority| authority.carrier()) else {
        return Response::err(
            ctx.req_id,
            format!("{CATALOG_ASSET_NOT_FOUND}: no usage is recorded for this asset"),
        );
    };
    let show_principals = carrier
        .scopes()
        .iter()
        .any(|scope| scope == CATALOG_USAGE_PRINCIPALS_SCOPE || scope == "*");
    let series = match read_usage_series(ctx, carrier.tenant_scope(), &asset).await {
        Ok(series) => series,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    match usage_answer(
        &asset,
        &series,
        top_n as usize,
        CATALOG_USAGE_SUGGESTION_MIN_COUNT.value(),
        show_principals,
    ) {
        Ok(answer) => Response::ok(ctx.req_id, ResultPayload::of::<CatalogUsage>(answer)),
        Err(refusal) => Response::err(ctx.req_id, refusal),
    }
}

/// The usage series of `asset` in the caller's tenant and graph, each with its summed count.
#[cfg(feature = "tsdb")]
async fn read_usage_series(
    ctx: &QueryHandlerCtx<'_>,
    tenant: &str,
    asset: &str,
) -> Result<Vec<(String, u64)>, String> {
    use medulla_tsdb::store::SeriesKey;
    let store = ctx.state.read().await.tsdb_store.clone();
    let Some(store) = store else {
        return Err(format!(
            "{CATALOG_ASSET_NOT_FOUND}: no usage is recorded for this asset"
        ));
    };
    let catalog = store
        .scoped_catalog(tenant, ctx.graph_name)
        .map_err(|_| "ENGINE_UNAVAILABLE: the usage store could not be read".to_string())?;
    let mut out = Vec::new();
    for name in catalog.into_keys() {
        let about = [
            ASSET_USAGE_READS_SERIES,
            ASSET_USAGE_JOIN_SERIES,
            ASSET_USAGE_FILTER_SERIES,
        ]
        .iter()
        .any(|base| series_tags(&name, base, asset).is_some());
        if !about {
            continue;
        }
        let key = SeriesKey::new(tenant, ctx.graph_name, name.clone());
        let points = store
            .scan_all_scoped(&key)
            .map_err(|_| "ENGINE_UNAVAILABLE: the usage store could not be read".to_string())?;
        let total: f64 = points
            .iter()
            .map(|p| p.values.first().copied().unwrap_or(0.0))
            .sum();
        out.push((name, total.max(0.0) as u64));
    }
    Ok(out)
}

/// Without the time-series build no usage exists.
#[cfg(not(feature = "tsdb"))]
async fn read_usage_series(
    _ctx: &QueryHandlerCtx<'_>,
    _tenant: &str,
    _asset: &str,
) -> Result<Vec<(String, u64)>, String> {
    Err(format!(
        "{CATALOG_ASSET_NOT_FOUND}: no usage is recorded for this asset"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::asset_usage::{
        usage_filter_series_name, usage_join_series_name, usage_series_name,
    };

    fn fixture() -> Vec<(String, u64)> {
        vec![
            (
                usage_series_name(ASSET_USAGE_READS_SERIES, "alice", "orders"),
                9,
            ),
            (
                usage_series_name(ASSET_USAGE_READS_SERIES, "bob", "orders"),
                4,
            ),
            (
                usage_series_name(ASSET_USAGE_READS_SERIES, "bob", "orders_archive"),
                100,
            ),
            (usage_join_series_name("alice", "orders", "customers"), 7),
            (usage_join_series_name("bob", "orders", "customers"), 1),
            (usage_join_series_name("bob", "orders", "items"), 2),
            (usage_filter_series_name("alice", "orders", "status"), 5),
        ]
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.8
    #[test]
    fn frequent_users_and_suggestions_rank_and_threshold() {
        let answer = usage_answer("orders", &fixture(), 5, 3, true).unwrap();
        assert_eq!(
            answer.frequent_users,
            vec![
                FrequentUser {
                    principal: Some("alice".into()),
                    reads: 9
                },
                FrequentUser {
                    principal: Some("bob".into()),
                    reads: 4
                },
            ]
        );
        let kinds: Vec<(&str, &str, u64)> = answer
            .suggestions
            .iter()
            .map(|s| (s.kind.as_str(), s.target.as_str(), s.count))
            .collect();
        // customers: 7 + 1 = 8; items: 2 is under the minimum of 3.
        assert_eq!(
            kinds,
            vec![("join", "customers", 8), ("filter", "status", 5)]
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.8
    #[test]
    fn principals_are_null_without_the_scope_and_unknown_assets_are_refused() {
        let hidden = usage_answer("orders", &fixture(), 1, 3, false).unwrap();
        assert_eq!(
            hidden.frequent_users,
            vec![FrequentUser {
                principal: None,
                reads: 9
            }]
        );
        let refused = usage_answer("nope", &fixture(), 5, 3, true).unwrap_err();
        assert!(refused.starts_with("CATALOG_ASSET_NOT_FOUND"));
    }
}
