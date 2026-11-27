//! The nearest-rank latency statistics every benchmark receipt reports. Included by `#[path]`
//! next to the receipt shape that reports them.

/// The nearest-rank percentile of an ascending-sorted slice.
pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

pub fn sorted(samples: &[f64]) -> Vec<f64> {
    let mut copy = samples.to_vec();
    copy.sort_by(f64::total_cmp);
    copy
}
