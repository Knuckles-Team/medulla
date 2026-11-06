//! MEDULLA-ORCH-VOCAB-R804: the tenant telemetry snapshot, `{metric name: latest
//! value}` over the last `telemetry.snapshot_window` seconds.
//!
//! It reads the metric samples the observability store already holds (the
//! `medulla_tenant` label is the convention `TelemetryDerive` and the collector
//! writers share) and derives nothing: the value of a name is the most recent
//! stored sample of any of the tenant's series with that name.

use std::collections::BTreeMap;

use super::ObsState;

/// The label every metric series is selected by (the writer convention).
const TENANT_LABEL: &str = super::writer::TENANT_ATTRIBUTE;
/// The label holding the metric name.
const NAME_LABEL: &str = "__name__";
/// Log series share the series store with metrics; they are not metrics.
const LOG_SERIES_PREFIX: &str = "obs:logs:";
const NANOS_PER_MILLI: i64 = 1_000_000;

/// The caller tenant's metric map over `[now_ms - window_secs, now_ms]`.
pub fn tenant_metric_snapshot(
    obs: &ObsState,
    tenant: &str,
    now_ms: u64,
    window_secs: u64,
) -> Result<BTreeMap<String, f64>, String> {
    let store = obs.series_store();
    let to_ns = i64::try_from(now_ms)
        .unwrap_or(i64::MAX)
        .saturating_mul(NANOS_PER_MILLI);
    let from_ns = to_ns.saturating_sub(
        i64::try_from(window_secs)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000)
            .saturating_mul(NANOS_PER_MILLI),
    );
    let series = store
        .list_series()
        .map_err(|_| "telemetry metric series are unavailable".to_string())?;
    // name -> (timestamp of the kept sample, value)
    let mut latest: BTreeMap<String, (i64, f64)> = BTreeMap::new();
    for series_id in series
        .iter()
        .filter(|id| !id.starts_with(LOG_SERIES_PREFIX))
    {
        let labels = crate::promql::parse_series_id(series_id);
        if labels.get(TENANT_LABEL).map(String::as_str) != Some(tenant) {
            continue;
        }
        let Some(name) = labels.get(NAME_LABEL) else {
            continue;
        };
        let points = store
            .range(series_id, from_ns, to_ns.saturating_add(1))
            .map_err(|_| "telemetry metric samples are unavailable".to_string())?;
        for point in &points {
            let Some(value) = point.values.first().copied() else {
                continue;
            };
            let newer = latest
                .get(name)
                .is_none_or(|(kept_ts, _)| point.ts >= *kept_ts);
            if newer {
                latest.insert(name.clone(), (point.ts, value));
            }
        }
    }
    Ok(latest
        .into_iter()
        .map(|(name, (_, value))| (name, value))
        .collect())
}
