//! The `DECAY HALF_LIFE` and `CLUSTER BY EMBEDDING` stages (MEDULLA-FEDERATED-QUERY-R720.2,
//! R720.3) and their refusals (R720.5).
//!
//! Both stages check every input row before they emit any: a row without a valid time
//! refuses `DECAY` with `UQL_E_DECAY_NO_TIME`, a row without a stored vector refuses
//! `CLUSTER BY EMBEDDING` with `UQL_E_CLUSTER_NO_VECTOR`. A refused stage is an `Err`, so
//! the program returns no partial rows.

use medulla_mining::mining::cluster::{cluster, Algorithm};
use medulla_types::contract::ServerErrorCode;

use super::PlanCtx;
use crate::rowset::{Row, RowSet};

/// The value channel naming each row's cluster.
pub const CLUSTER_ID_CHANNEL: &str = "cluster_id";
/// The value channel holding each row's cosine distance to its cluster centroid.
pub const CENTROID_DISTANCE_CHANNEL: &str = "centroid_distance";

/// The EM rounds the seeded clustering runs (fixed, so results repeat).
const CLUSTER_MAX_ITER: usize = 100;
/// The clustering seed: fixed at 0 so the same rows always give the same clusters.
const CLUSTER_SEED: u64 = 0;

fn refusal(code: ServerErrorCode, detail: &str) -> String {
    format!("{}: {detail}", code.as_str())
}

/// A row's valid time (epoch ms): the row's own field, else the node's stored
/// `valid_time_ms` property.
fn valid_time_ms(ctx: &PlanCtx, row: &Row) -> Option<i64> {
    row.valid_time_ms.or_else(|| {
        let blob = ctx.view.node_properties.get(row.id.as_str())?;
        let value = medulla_types::msgpack::decode_property_value(blob.as_slice()).ok()?;
        value.get("valid_time_ms").and_then(|v| v.as_i64())
    })
}

fn now_ms(ctx: &PlanCtx) -> i64 {
    ctx.now_ms.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    })
}

/// `DECAY HALF_LIFE <seconds>`: each row's score (1.0 when unscored) times
/// `0.5^(age / half_life)`, age = now − valid time in seconds (the half-life weight of
/// `medulla-numeric`'s reputation learner). Row order and value channels are kept.
pub(super) fn decay(ctx: &PlanCtx, input: RowSet, half_life_secs: f64) -> Result<RowSet, String> {
    if !(half_life_secs.is_finite() && half_life_secs > 0.0) {
        return Err("DECAY HALF_LIFE must be a finite number of seconds > 0".into());
    }
    let now = now_ms(ctx);
    let times: Vec<i64> = input
        .rows()
        .iter()
        .map(|row| {
            valid_time_ms(ctx, row).ok_or_else(|| {
                refusal(
                    ServerErrorCode::UqlEDecayNoTime,
                    &format!("row `{}` has no valid time", row.id),
                )
            })
        })
        .collect::<Result<_, _>>()?;
    let (rows, values) = input.into_parts();
    let decayed = rows.into_iter().zip(times).map(|(mut row, t)| {
        let age_secs = (now - t) as f64 / 1000.0;
        let weight = 0.5f64.powf(age_secs / half_life_secs);
        row.score = Some((f64::from(row.score.unwrap_or(1.0)) * weight) as f32);
        row
    });
    Ok(RowSet::from_epistemic_rows(decayed).with_values(values))
}

fn unit(v: &[f32]) -> Vec<f64> {
    let norm = v
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return v.iter().map(|_| 0.0).collect();
    }
    v.iter().map(|x| f64::from(*x) / norm).collect()
}

fn cosine_distance(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        1.0
    } else {
        1.0 - dot / (na * nb)
    }
}

/// `CLUSTER BY EMBEDDING K <n>`: the existing mining `cluster` (seeded mixture, seed 0)
/// over the rows' unit-normalised embeddings, so the partition follows cosine distance.
/// Rows keep their order; each gains `cluster_id` and `centroid_distance` (cosine).
pub(super) fn cluster_by_embedding(
    ctx: &PlanCtx,
    input: RowSet,
    k: usize,
) -> Result<RowSet, String> {
    if k == 0 {
        return Err("CLUSTER BY EMBEDDING needs K >= 1".into());
    }
    let points: Vec<Vec<f64>> = input
        .rows()
        .iter()
        .map(|row| {
            ctx.semantic
                .get_embedding(&row.id)
                .filter(|v| !v.is_empty())
                .map(|v| unit(&v))
                .ok_or_else(|| {
                    refusal(
                        ServerErrorCode::UqlEClusterNoVector,
                        &format!("row `{}` has no vector", row.id),
                    )
                })
        })
        .collect::<Result<_, _>>()?;
    if let Some(dim) = points.first().map(Vec::len) {
        if points.iter().any(|p| p.len() != dim) {
            return Err(refusal(
                ServerErrorCode::UqlEClusterNoVector,
                "rows carry vectors of different dimensions",
            ));
        }
    }
    let k = k.min(points.len().max(1));
    let clustering = cluster(
        &points,
        Algorithm::Gmm {
            k,
            max_iter: CLUSTER_MAX_ITER,
            seed: CLUSTER_SEED,
        },
    );
    let (rows, mut values) = input.into_parts();
    for (i, row) in rows.iter().enumerate() {
        let label = clustering.labels[i];
        let distance = clustering
            .clusters
            .iter()
            .find(|c| c.cluster_id == label)
            .map_or(1.0, |c| cosine_distance(&points[i], &c.centroid));
        let channels = values.entry(row.id.clone()).or_default();
        channels.insert(CLUSTER_ID_CHANNEL.to_string(), label as f64);
        channels.insert(CENTROID_DISTANCE_CHANNEL.to_string(), distance);
    }
    Ok(RowSet::from_epistemic_rows(rows).with_values(values))
}
